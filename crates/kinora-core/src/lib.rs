//! Kinora's core: everything the app knows that is not about how it looks.
//!
//! No GUI toolkit is used here (no gtk, gdk, glib or adw). Frontends (the GTK
//! app in `src/`) read their own settings, pass plain values in
//! ([`AccountConfig`], [`PlaybackConfig`], [`Paths`]), and turn the results
//! ([`pcore::Video`], [`pcore::Channel`], [`home::HomePage`], [`PlaylistCard`]) into their own
//! widgets and models. See `docs/architecture.md`.
//!
//! [`Core`] is the entry point. Its futures need a Tokio runtime (they run
//! child processes and make HTTP requests); they are `Send`, so a frontend can
//! spawn them on its runtime and hand the results to its UI thread.

pub mod account;
pub mod config;
pub mod dj;
pub mod extension;
pub mod helper;
pub mod home;
pub mod player;
pub mod playlist;
pub mod streams;
pub mod youtube;

use std::time::Duration;

use pcore::{Video, VideoId};

pub use config::{AccountConfig, Paths, PlaybackConfig};
use dj::{
    DjSeed, DjSession, DjSettings, RadioPage, RadioRequest, StartFrom, Track, TransitionPlan,
};
use extension::{
    ActionOutcome, Context, ExtensionRegistry, FeedPlacement, FeedRequest, MenuTarget, Permission,
    SettingValue, builtin,
};
pub use home::{HomeChip, HomePage};
use player::Transition;
pub use playlist::{PlaylistCard, PlaylistKind};

/// Errors are messages for the user or the log; nothing matches on them yet.
pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// What frontends talk to. Share one in an `Arc`.
#[derive(Debug)]
pub struct Core {
    paths: Paths,
    extensions: ExtensionRegistry,
}

impl Core {
    /// A core with the built-in extensions, all enabled.
    pub fn new(paths: Paths) -> Self {
        Self::with_extensions(paths, ExtensionRegistry::with_builtins())
    }

    pub fn with_extensions(paths: Paths, extensions: ExtensionRegistry) -> Self {
        Self { paths, extensions }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// The extensions, to switch them on and off, pass their stored settings
    /// in, and list them in the preferences.
    pub fn extensions(&self) -> &ExtensionRegistry {
        &self.extensions
    }

    /// Whether the account is in use: connected, and its extension enabled.
    /// Without it Kinora is fully anonymous.
    pub fn account_active(&self, account: &AccountConfig) -> bool {
        account.is_connected() && self.extensions.is_enabled(builtin::account::ID)
    }

    // ---- Home ----

    /// The first page of Home, from the first enabled main feed source (the
    /// account extension's YouTube Home): the plain home page, or the feed of
    /// the chip with this continuation token.
    ///
    /// For a list of videos: pages without plain videos are skipped. The
    /// playlists and mixes of the pages it read are in
    /// [`HomePage::playlists`]; to show them, use
    /// [`Self::home_with_playlists`].
    pub async fn home(
        &self,
        account: &AccountConfig,
        chip_token: Option<&str>,
    ) -> Result<HomePage, Error> {
        self.home_first(account, chip_token, false).await
    }

    /// The next page of Home, from the previous page's [`HomePage::next`].
    pub async fn home_more(&self, account: &AccountConfig, token: &str) -> Result<HomePage, Error> {
        self.home_next(account, token, false).await
    }

    /// [`Self::home`] for a frontend that shows playlist and mix cards
    /// ([`HomePage::playlists`]) next to the videos: a page with playlists
    /// and no videos (the "Mixes" and "Playlists" chips) is a page to show,
    /// not an empty one to skip.
    pub async fn home_with_playlists(
        &self,
        account: &AccountConfig,
        chip_token: Option<&str>,
    ) -> Result<HomePage, Error> {
        self.home_first(account, chip_token, true).await
    }

    /// [`Self::home_more`] for a frontend that shows playlists (see
    /// [`Self::home_with_playlists`]).
    pub async fn home_more_with_playlists(
        &self,
        account: &AccountConfig,
        token: &str,
    ) -> Result<HomePage, Error> {
        self.home_next(account, token, true).await
    }

    async fn home_first(
        &self,
        account: &AccountConfig,
        chip_token: Option<&str>,
        playlists: bool,
    ) -> Result<HomePage, Error> {
        let request = FeedRequest {
            chip: chip_token.map(str::to_owned),
            continuation: None,
            playlists,
        };
        self.load_home(account, request).await.map_err(|e| {
            e.unwrap_or_else(|| {
                "No YouTube account is set up. Set the account cookies command in the preferences."
                    .into()
            })
        })
    }

    async fn home_next(
        &self,
        account: &AccountConfig,
        token: &str,
        playlists: bool,
    ) -> Result<HomePage, Error> {
        let request = FeedRequest {
            chip: None,
            continuation: Some(token.to_owned()),
            playlists,
        };
        self.load_home(account, request)
            .await
            .map_err(|e| e.unwrap_or_else(|| "No YouTube account is set up.".into()))
    }

    /// `Err(None)`: no feed source is available.
    async fn load_home(
        &self,
        account: &AccountConfig,
        request: FeedRequest,
    ) -> Result<HomePage, Option<Error>> {
        let (source, ctx) = self
            .extensions
            .feed_source(FeedPlacement::Main, account, &self.paths)
            .ok_or(None)?;
        let mut page = source.load(ctx, request).await.map_err(Some)?;
        // Chips are the home-chips extension's: without it, Home is one feed.
        if !self.extensions.is_enabled(builtin::home_chips::ID) {
            page.chips.clear();
        }
        self.extensions.filter_feed(&mut page, account, &self.paths);
        Ok(page)
    }

    // ---- The account ----

    /// YouTube's "Up next" for a video: personalised when the account is
    /// active (and signing in works), otherwise the anonymous related videos.
    pub async fn up_next(
        &self,
        account: &AccountConfig,
        video: &VideoId,
    ) -> Result<Vec<Video>, Error> {
        use builtin::account::{ID, PERSONAL_UP_NEXT};
        if !youtube::is_youtube(video) {
            return Ok(Vec::new());
        }
        let personal = self.account_active(account)
            && self.extensions.value(ID, PERSONAL_UP_NEXT) == Some(SettingValue::Bool(true));
        let cookies = if personal {
            account.cookies().await.unwrap_or_else(|e| {
                log::warn!("Up next: {e}");
                None
            })
        } else {
            None
        };
        account::up_next(self.paths.rustypipe_dir(), cookies, video.id.clone()).await
    }

    /// The signed-in account's playlists, as YouTube's "Playlists" page
    /// lists them (its own and the saved ones), plus "Liked videos"
    /// ([`playlist::LIKED_VIDEOS`]) and "Watch later"
    /// ([`playlist::WATCH_LATER`]) when YouTube leaves them out. Play one with
    /// [`PlaylistCard::dj_seed`] and [`Self::dj_start`] (the DJ fetches it
    /// signed in, so private playlists play too).
    ///
    /// Fails without an active account ([`Self::account_active`]).
    pub async fn account_playlists(
        &self,
        account: &AccountConfig,
    ) -> Result<Vec<PlaylistCard>, Error> {
        if !self.account_active(account) {
            return Err("No YouTube account is in use. Connect one in the preferences.".into());
        }
        let cookies = account
            .cookies()
            .await?
            .ok_or("No YouTube account is set up.")?;
        account::playlists(self.paths.rustypipe_dir(), cookies).await
    }

    // ---- Extension contributions ----

    /// A video has really been watched: run the enabled extensions' hooks
    /// (the account extension marks it watched on YouTube).
    pub async fn watched(&self, account: &AccountConfig, video: &Video) {
        self.extensions.watched(video, account, &self.paths).await;
    }

    /// A video played to its end: run the enabled extensions' hooks.
    pub async fn ended(&self, account: &AccountConfig, video: &Video) {
        self.extensions.ended(video, account, &self.paths).await;
    }

    /// Which Up next video plays after `current`, if an extension decides;
    /// `None` means the first one.
    pub fn choose_next(
        &self,
        account: &AccountConfig,
        current: &Video,
        candidates: &[Video],
    ) -> Option<usize> {
        self.extensions
            .choose_next(current, candidates, account, &self.paths)
    }

    /// How to go from `current` to `next` (a cut unless an extension says otherwise).
    pub fn transition(&self, account: &AccountConfig, current: &Video, next: &Video) -> Transition {
        self.extensions
            .transition(current, next, account, &self.paths)
    }

    /// Run an extension's menu item (see [`ExtensionRegistry::menu_items`]).
    pub async fn activate(
        &self,
        account: &AccountConfig,
        extension: &str,
        item: &str,
        target: MenuTarget,
    ) -> Result<ActionOutcome, Error> {
        self.extensions
            .activate(extension, item, target, account, &self.paths)
            .await
    }

    // ---- DJ ----

    /// The DJ extension's context; `Err` while it is switched off.
    fn dj_context(&self, account: &AccountConfig) -> Result<Context, Error> {
        if !self.extensions.is_enabled(builtin::dj::ID) {
            return Err("The DJ is switched off. Turn it on in the preferences.".into());
        }
        let ctx = self
            .extensions
            .context(builtin::dj::ID, account, &self.paths)
            .ok_or("The DJ is not installed")?;
        ctx.require(Permission::Network)?;
        ctx.require(Permission::PlayerControl)?;
        Ok(ctx)
    }

    /// The DJ's settings (crossfade, avoid repeats, start from), defaults
    /// filled in.
    pub fn dj_settings(&self) -> DjSettings {
        self.extensions
            .context(builtin::dj::ID, &AccountConfig::anonymous(), &self.paths)
            .map(|ctx| builtin::dj::settings(&ctx))
            .unwrap_or_default()
    }

    /// Where "DJ" starts, following the "start from" setting: the open
    /// YouTube video, or "my mix". "My mix" needs the account; without it
    /// the open video is used when there is one.
    pub fn dj_seed(&self, account: &AccountConfig, current: Option<&VideoId>) -> DjSeed {
        let video = current
            .filter(|v| youtube::is_youtube(v))
            .map(|v| DjSeed::Video(v.id.clone()));
        match (self.dj_settings().start_from, video) {
            (StartFrom::MyMix, Some(video)) if !self.account_active(account) => video,
            (StartFrom::MyMix, _) | (StartFrom::CurrentVideo, None) => DjSeed::MyMix,
            (StartFrom::CurrentVideo, Some(video)) => video,
        }
    }

    /// Start a DJ session: the first page of the seed's radio, in a session
    /// set up from the DJ's settings. The radio is personal when the account
    /// is active. See [`dj`] for the loop a frontend runs.
    pub async fn dj_start(
        &self,
        account: &AccountConfig,
        seed: DjSeed,
    ) -> Result<DjSession, Error> {
        let request = RadioRequest::Start(seed.clone());
        let page = self.dj_more(account, &request).await?;
        Ok(DjSession::start(seed, self.dj_settings().options(), page))
    }

    /// Fetch a page for a session: [`DjSession::refill_request`]'s answer.
    /// Hand it to [`DjSession::extend`] (or, on an error, tell the session
    /// with [`DjSession::fetch_failed`]). Takes no session, so a frontend
    /// need not hold its session across the request.
    pub async fn dj_more(
        &self,
        account: &AccountConfig,
        request: &RadioRequest,
    ) -> Result<RadioPage, Error> {
        let ctx = self.dj_context(account)?;
        let cookies = if self.account_active(account) {
            match ctx.account() {
                Ok(account) => account.cookies().await.unwrap_or_else(|e| {
                    log::warn!("DJ: {e}");
                    None
                }),
                Err(_) => None,
            }
        } else {
            None
        };
        dj::radio::fetch(request, cookies.as_deref()).await
    }

    /// Top a session's queue up (a few requests at most, until it no longer
    /// [`needs_refill`](DjSession::needs_refill)). Returns how many tracks
    /// were added; an error only when nothing could be added. For frontends
    /// that can lend the session to a future; others use [`Self::dj_more`].
    pub async fn dj_refill(
        &self,
        account: &AccountConfig,
        session: &mut DjSession,
    ) -> Result<usize, Error> {
        const MAX_REQUESTS: usize = 3;
        let mut added = 0;
        let mut error = None;
        for _ in 0..MAX_REQUESTS {
            let Some(request) = session.refill_request() else {
                break;
            };
            match self.dj_more(account, &request).await {
                Ok(page) => added += session.extend(&request, page),
                Err(e) => {
                    session.fetch_failed(&request);
                    error = Some(e);
                }
            }
            if !session.needs_refill() {
                break;
            }
        }
        match error {
            Some(e) if added == 0 => Err(e),
            _ => Ok(added),
        }
    }

    /// When and how to go from the current track to `next`, with the
    /// crossfade setting: see [`dj::plan_transition`]. `duration` is the
    /// current track's length as the player knows it (or its
    /// [`Track::duration`]), `position` where it is now.
    pub fn dj_transition(
        &self,
        duration: Option<Duration>,
        position: Duration,
        next: &Track,
    ) -> TransitionPlan {
        dj::plan_transition(
            duration,
            position,
            next.duration,
            self.dj_settings().crossfade,
        )
    }

    // ---- Playback ----

    /// Experimental fast start: a URI a player can open right away (a local
    /// DASH manifest, or HLS for live streams). See [`streams`].
    pub async fn fast_playback_uri(&self, video: &VideoId) -> Result<String, Error> {
        if !youtube::is_youtube(video) {
            return Err("fast start is for YouTube videos only".into());
        }
        streams::playback_uri(
            self.paths.rustypipe_dir(),
            self.paths.streams_dir(),
            video.id.clone(),
        )
        .await
    }

    /// Look a video's streams up ahead of time, so it starts at once when
    /// picked. Afterwards [`Self::prepared_uri`] has it.
    pub async fn prepare(&self, playback: &PlaybackConfig, video: &VideoId) -> Result<(), Error> {
        if !youtube::is_youtube(video) {
            return Ok(());
        }
        if playback.fast_start {
            self.fast_playback_uri(video).await.map(|_| ())
        } else {
            streams::prepare_with_ytdlp(self.paths.streams_dir(), video.id.clone()).await
        }
    }

    /// A prepared playback URI for this video, if there is a fresh one.
    pub fn prepared_uri(&self, video: &VideoId) -> Option<String> {
        if !youtube::is_youtube(video) {
            return None;
        }
        streams::prepared(&video.id)
    }

    /// Like [`Self::prepare`], audio only, in the original language (the DJ
    /// plays music this way): starts in about half the time and decodes no
    /// video. Afterwards [`Self::prepared_audio_uri`] has it.
    pub async fn prepare_audio(&self, video: &VideoId) -> Result<(), Error> {
        if !youtube::is_youtube(video) {
            return Ok(());
        }
        streams::prepare_audio_with_ytdlp(self.paths.streams_dir(), video.id.clone()).await
    }

    /// A prepared audio-only playback URI for this video, if there is a fresh one.
    pub fn prepared_audio_uri(&self, video: &VideoId) -> Option<String> {
        if !youtube::is_youtube(video) {
            return None;
        }
        streams::prepared_audio(&video.id)
    }
}
