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
pub mod home;
pub mod player;
pub mod streams;
pub mod youtube;

use pcore::{Channel, Video, VideoId};

pub use config::{AccountConfig, Paths, PlaybackConfig};
pub use home::{HomeChip, HomePage};

/// Errors are messages for the user or the log; nothing matches on them yet.
pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// What frontends talk to. Cheap to share: keep one in an `Arc`.
#[derive(Debug)]
pub struct Core {
    paths: Paths,
}

impl Core {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    // ---- Home ----

    /// The first page of the account's Home: the plain home page, or the feed
    /// of the chip with this continuation token.
    pub async fn home(
        &self,
        account: &AccountConfig,
        chip_token: Option<&str>,
    ) -> Result<HomePage, Error> {
        let cookies = account.cookies().await?.ok_or(
            "No YouTube account is set up. Set the account cookies command in the preferences.",
        )?;
        home::load(&cookies, chip_token).await
    }

    /// The next page of Home, from the previous page's [`HomePage::next`].
    pub async fn home_more(&self, account: &AccountConfig, token: &str) -> Result<HomePage, Error> {
        let cookies = account
            .cookies()
            .await?
            .ok_or("No YouTube account is set up.")?;
        home::continuation(&cookies, token).await
    }

    // ---- The account ----

    /// YouTube's "Up next" for a video: personalised when an account is set up
    /// (and signing in works), otherwise the anonymous related videos.
    pub async fn up_next(
        &self,
        account: &AccountConfig,
        video: &VideoId,
    ) -> Result<Vec<Video>, Error> {
        if !youtube::is_youtube(video) {
            return Ok(Vec::new());
        }
        let cookies = account.cookies().await.unwrap_or_else(|e| {
            log::warn!("Up next: {e}");
            None
        });
        account::up_next(self.paths.rustypipe_dir(), cookies, video.id.clone()).await
    }

    /// Every channel the account is subscribed to on YouTube, in YouTube's order.
    pub async fn account_subscriptions(
        &self,
        account: &AccountConfig,
    ) -> Result<Vec<Channel>, Error> {
        let cookies = account.cookies().await?.ok_or(
            "No YouTube account is set up. Set the account cookies command in the preferences.",
        )?;
        account::subscriptions(self.paths.rustypipe_dir(), cookies).await
    }

    /// Mark a video watched on the account, so its recommendations learn from
    /// it. `Ok(false)` when there is no account (or it is not a YouTube video).
    pub async fn mark_watched(
        &self,
        account: &AccountConfig,
        video: &VideoId,
    ) -> Result<bool, Error> {
        if !youtube::is_youtube(video) {
            return Ok(false);
        }
        match account.cookies().await? {
            Some(cookies) => {
                account::mark_watched(&cookies, &video.id).await?;
                Ok(true)
            }
            None => Ok(false),
        }
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
