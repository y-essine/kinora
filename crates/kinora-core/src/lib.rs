//! Kinora's core: everything the app knows that is not about how it looks.
//!
//! No GUI toolkit is used here (no gtk, gdk, glib or adw). Frontends (the GTK
//! app in `src/`) read their own settings, pass plain values in
//! ([`AccountConfig`], [`PlaybackConfig`], [`Paths`]), and turn the results
//! ([`pcore::Video`], [`pcore::Channel`], [`home::HomePage`]) into their own
//! widgets and models. See `docs/architecture.md`.
//!
//! [`Core`] is the entry point. Its futures need a Tokio runtime (they run
//! child processes and make HTTP requests); they are `Send`, so a frontend can
//! spawn them on its runtime and hand the results to its UI thread.

pub mod account;
pub mod config;
pub mod extension;
pub mod home;
pub mod player;
pub mod streams;
pub mod youtube;

use pcore::{Video, VideoId};

pub use config::{AccountConfig, Paths, PlaybackConfig};
use extension::{
    ActionOutcome, ExtensionRegistry, FeedPlacement, FeedRequest, MenuTarget, SettingValue, builtin,
};
pub use home::{HomeChip, HomePage};
use player::Transition;

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
    pub async fn home(
        &self,
        account: &AccountConfig,
        chip_token: Option<&str>,
    ) -> Result<HomePage, Error> {
        let request = FeedRequest {
            chip: chip_token.map(str::to_owned),
            continuation: None,
        };
        self.load_home(account, request).await.map_err(|e| {
            e.unwrap_or_else(|| {
                "No YouTube account is set up. Set the account cookies command in the preferences."
                    .into()
            })
        })
    }

    /// The next page of Home, from the previous page's [`HomePage::next`].
    pub async fn home_more(&self, account: &AccountConfig, token: &str) -> Result<HomePage, Error> {
        let request = FeedRequest {
            chip: None,
            continuation: Some(token.to_owned()),
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
}
