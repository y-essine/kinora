//! The DJ: a continuous music session from a seed (a video, "my mix", or a
//! playlist, album or mix: [`DjSeed`]), with no repeats and crossfades
//! between tracks. This is the engine; the frontend draws it and plays the
//! audio.
//!
//! - [`radio`]: fetches the songs (YouTube Music's radio or playlist,
//!   regular YouTube's mix or playlist as the fallback) and parses them into
//!   [`Track`]s.
//! - [`DjSession`]: the queue (current, upcoming, history), skip, "play
//!   next", remove, dedupe, and when to fetch more.
//! - [`plan_transition`]: when to start the next track and the crossfade
//!   curve ([`TransitionPlan`]), from the tracks' lengths and the setting.
//!
//! The `dj` built-in extension ([`crate::extension::builtin::dj`]) owns the
//! settings and the permissions; [`crate::Core`] has the entry points.
//!
//! # A frontend's loop
//!
//! ```no_run
//! # async fn example(core: &kinora_core::Core, account: &kinora_core::AccountConfig)
//! # -> Result<(), kinora_core::Error> {
//! use std::time::Duration;
//! use kinora_core::dj::DjSeed;
//!
//! // Start from the video on screen (or `core.dj_seed(..)` for the setting).
//! let mut session = core.dj_start(account, DjSeed::Video("dQw4w9WgXcQ".into())).await?;
//!
//! // While a track plays: prepare the next one, plan the crossfade.
//! let (current, next) = (session.current().unwrap(), session.peek_next().unwrap());
//! core.prepare(&Default::default(), &next.video_id()).await?;
//! let plan = core.dj_transition(current.duration, Duration::from_secs(12), next);
//! // ...when `plan.should_start(position)`: start `next` at volume 0, then
//! // every ~50 ms set both volumes from `plan.gains(elapsed)` until
//! // `plan.fade` has passed, and stop the old one. A cut: wait for the end.
//!
//! // When the next track took over:
//! session.advance(); // or `session.skip()` when the user skipped
//!
//! // Keep the queue topped up (in the background, without holding the session):
//! if let Some(request) = session.refill_request() {
//!     let page = core.dj_more(account, &request).await?;
//!     session.extend(&request, page);
//! }
//! # Ok(())
//! # }
//! ```

mod dedupe;
pub mod radio;
mod session;
mod track;
mod transition;

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use dedupe::{same_song, track_key};
pub use radio::{DjSeed, RadioContinuation, RadioPage, RadioRequest, RadioSource};
pub use session::{DEFAULT_REFILL_BELOW, DjOptions, DjSession, HistoryEntry};
pub use track::{Album, Artist, Track, TrackKind};
pub use transition::{
    DEFAULT_CROSSFADE, FadeGains, FadePoint, MAX_CROSSFADE, MIN_FADE, MIN_TRACK_FOR_FADE,
    TransitionPlan, equal_power, plan_transition,
};

/// Where a session starts when the user just says "DJ" (the "start from"
/// setting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartFrom {
    /// The video that is open (or "my mix" when none is).
    #[default]
    CurrentVideo,
    /// The account's mix (YouTube's popular mix without an account).
    MyMix,
}

/// The DJ extension's settings, read from the registry (see
/// [`crate::Core::dj_settings`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DjSettings {
    /// The wanted crossfade, 0 to [`MAX_CROSSFADE`]; zero means cuts.
    pub crossfade: Duration,
    pub avoid_repeats: bool,
    pub start_from: StartFrom,
}

impl Default for DjSettings {
    fn default() -> Self {
        Self {
            crossfade: DEFAULT_CROSSFADE,
            avoid_repeats: true,
            start_from: StartFrom::CurrentVideo,
        }
    }
}

impl DjSettings {
    /// The session options these settings give.
    pub fn options(&self) -> DjOptions {
        DjOptions {
            avoid_repeats: self.avoid_repeats,
            ..DjOptions::default()
        }
    }
}
