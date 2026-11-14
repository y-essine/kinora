//! Player hooks: extensions react to playback and steer what plays next.

use futures::future::{BoxFuture, ready};
use pcore::Video;

use super::Context;
use crate::Error;
use crate::player::Transition;

/// Reacts to playback. Every method has a do-nothing default, so a hook
/// implements only what it needs.
///
/// `choose_next` and `transition` are only asked of extensions with the
/// [`PlayerControl`](super::Permission::PlayerControl) permission.
pub trait PlayerHook: Send + Sync {
    /// The video has really been watched (the GTK app: still on its page 30 s
    /// after it started, or opened in an external player).
    fn on_watched(&self, _ctx: Context, _video: Video) -> BoxFuture<'static, Result<(), Error>> {
        Box::pin(ready(Ok(())))
    }

    /// The video played to its end.
    fn on_ended(&self, _ctx: Context, _video: Video) -> BoxFuture<'static, Result<(), Error>> {
        Box::pin(ready(Ok(())))
    }

    /// Pick what plays after `current`: an index into `candidates` (the Up
    /// next list). `None`: no opinion; the next hook, or the frontend's
    /// default (the first candidate), decides.
    fn choose_next(
        &self,
        _ctx: &Context,
        _current: &Video,
        _candidates: &[Video],
    ) -> Option<usize> {
        None
    }

    /// How to go from `current` to `next` (a crossfade for a DJ mode).
    /// `None`: no opinion. Players that can't do it fall back to a cut.
    fn transition(&self, _ctx: &Context, _current: &Video, _next: &Video) -> Option<Transition> {
        None
    }
}
