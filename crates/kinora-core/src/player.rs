//! The playback interface a frontend implements for its video player.
//!
//! The core decides *what* plays (a prepared URI, the fast-start manifest,
//! the next video); a [`Player`] decides *how*: Clapper (GStreamer) in the GTK
//! app, mpv or a web `<video>` element in other frontends. Everything here is
//! called on the frontend's UI thread, so implementations need not be `Send`.
//!
//! Gapless playback and crossfading are optional [`PlayerCapabilities`]: a
//! player that cannot do them keeps the default methods, and the core falls
//! back to an ordinary cut ([`Transition::Cut`]).

use std::fmt;
use std::time::Duration;

/// What a player is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerState {
    /// Nothing loaded, or playback was stopped.
    Stopped,
    /// Loading or waiting for data; will play when it has enough.
    Buffering,
    Paused,
    Playing,
}

/// What a player reports to the frontend (see [`Player::connect_event`]).
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    StateChanged(PlayerState),
    /// The length of the loaded media became known (or changed).
    DurationChanged(Duration),
    /// The media played to its end: not a pause, not a stop. Players whose
    /// backend has no such signal can use [`reached_end`].
    EndOfStream,
    /// Playback failed; the frontend may fall back to another URI.
    Error(String),
}

/// Optional abilities of a player.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerCapabilities {
    /// Can start a queued URI exactly when the current one ends ([`Player::queue_next`]).
    pub gapless: bool,
    /// Can fade from the current media into another ([`Player::crossfade_to`]).
    pub crossfade: bool,
}

/// How one video gives way to the next.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Transition {
    /// Stop, then load and play the next one (what every player can do).
    #[default]
    Cut,
    /// The next one starts with no gap (needs [`PlayerCapabilities::gapless`]).
    Gapless,
    /// Both play for this long while one fades out and the other in
    /// (needs [`PlayerCapabilities::crossfade`]).
    Crossfade(Duration),
}

/// Returned by the optional methods a player does not support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unsupported;

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not supported by this player")
    }
}

impl std::error::Error for Unsupported {}

/// Receives a player's events.
pub type EventHandler = Box<dyn Fn(&PlayerEvent)>;

/// A video player, as the core sees it. Positions and durations are
/// [`Duration`]s from the start of the media.
pub trait Player {
    /// Replace whatever is loaded with `uri` (`https://`, `file://`, ...). Does
    /// not start playing by itself.
    fn load(&self, uri: &str);
    fn play(&self);
    fn pause(&self);
    /// Stop and unload.
    fn stop(&self);
    /// Jump to `position` (clamped to the media by the player).
    fn seek(&self, position: Duration);
    fn position(&self) -> Duration;
    /// The loaded media's length; `None` while unknown, and for live streams.
    fn duration(&self) -> Option<Duration>;
    fn state(&self) -> PlayerState;
    /// Call `handler` for every [`PlayerEvent`] from now on. A player may
    /// support several handlers; callers must not rely on replacing one.
    fn connect_event(&self, handler: EventHandler);

    /// What this player can do beyond the basics.
    fn capabilities(&self) -> PlayerCapabilities {
        PlayerCapabilities::default()
    }

    /// Gapless: play `uri` right when the current media ends.
    fn queue_next(&self, _uri: &str) -> Result<(), Unsupported> {
        Err(Unsupported)
    }

    /// Crossfade from the current media into `uri` over `over`.
    fn crossfade_to(&self, _uri: &str, _over: Duration) -> Result<(), Unsupported> {
        Err(Unsupported)
    }
}

/// Seek relative to the current position, kept within the media.
pub fn seek_by(player: &dyn Player, seconds: f64) {
    let position = player.position().as_secs_f64() + seconds;
    let mut position = position.max(0.0);
    if let Some(duration) = player.duration() {
        position = position.min(duration.as_secs_f64());
    }
    player.seek(Duration::from_secs_f64(position));
}

/// How close to the end a stopped or paused player counts as finished.
pub const END_TOLERANCE: Duration = Duration::from_millis(750);

/// Whether a player that stopped or paused at `position` did so because the
/// media ended (and not because the user paused it). Live streams have no
/// end: pass `None` for their duration.
pub fn reached_end(position: Duration, duration: Option<Duration>) -> bool {
    match duration {
        Some(duration) if !duration.is_zero() => position + END_TOLERANCE >= duration,
        _ => false,
    }
}

/// Play `uri` after what is playing now, with `transition` if the player can
/// do it and an ordinary cut otherwise. Returns the transition really used.
pub fn play_next(player: &dyn Player, uri: &str, transition: Transition) -> Transition {
    let caps = player.capabilities();
    let done = match transition {
        Transition::Crossfade(over) if caps.crossfade => player.crossfade_to(uri, over).is_ok(),
        Transition::Gapless if caps.gapless => player.queue_next(uri).is_ok(),
        _ => false,
    };
    if done {
        return transition;
    }
    player.load(uri);
    player.play();
    Transition::Cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// A player that only remembers what it was told.
    #[derive(Default)]
    struct FakePlayer {
        uri: RefCell<Option<String>>,
        position: Cell<Duration>,
        duration: Cell<Option<Duration>>,
        state: Cell<Option<PlayerState>>,
        crossfade: bool,
        handlers: RefCell<Vec<EventHandler>>,
    }

    impl FakePlayer {
        fn emit(&self, event: PlayerEvent) {
            for handler in self.handlers.borrow().iter() {
                handler(&event);
            }
        }
    }

    impl Player for FakePlayer {
        fn load(&self, uri: &str) {
            self.uri.replace(Some(uri.to_owned()));
            self.position.set(Duration::ZERO);
        }
        fn play(&self) {
            self.state.set(Some(PlayerState::Playing));
            self.emit(PlayerEvent::StateChanged(PlayerState::Playing));
        }
        fn pause(&self) {
            self.state.set(Some(PlayerState::Paused));
        }
        fn stop(&self) {
            self.state.set(Some(PlayerState::Stopped));
            self.uri.replace(None);
        }
        fn seek(&self, position: Duration) {
            self.position.set(position);
        }
        fn position(&self) -> Duration {
            self.position.get()
        }
        fn duration(&self) -> Option<Duration> {
            self.duration.get()
        }
        fn state(&self) -> PlayerState {
            self.state.get().unwrap_or(PlayerState::Stopped)
        }
        fn connect_event(&self, handler: EventHandler) {
            self.handlers.borrow_mut().push(handler);
        }
        fn capabilities(&self) -> PlayerCapabilities {
            PlayerCapabilities {
                gapless: false,
                crossfade: self.crossfade,
            }
        }
        fn crossfade_to(&self, uri: &str, _over: Duration) -> Result<(), Unsupported> {
            if !self.crossfade {
                return Err(Unsupported);
            }
            self.uri.replace(Some(format!("crossfade:{uri}")));
            Ok(())
        }
    }

    #[test]
    fn seeking_stays_within_the_media() {
        let player = FakePlayer::default();
        player.duration.set(Some(Duration::from_secs(60)));
        player.seek(Duration::from_secs(5));
        seek_by(&player, -10.0);
        assert_eq!(player.position(), Duration::ZERO);
        seek_by(&player, 100.0);
        assert_eq!(player.position(), Duration::from_secs(60));
    }

    #[test]
    fn end_only_counts_at_the_very_end() {
        let minute = Some(Duration::from_secs(60));
        assert!(reached_end(Duration::from_millis(59_500), minute));
        assert!(!reached_end(Duration::from_secs(30), minute));
        // Live streams and unknown lengths never end.
        assert!(!reached_end(Duration::from_secs(60), None));
        assert!(!reached_end(Duration::from_secs(1), Some(Duration::ZERO)));
    }

    #[test]
    fn transitions_fall_back_to_a_cut() {
        let player = FakePlayer::default();
        let seen = std::rc::Rc::new(Cell::new(0));
        let counter = seen.clone();
        player.connect_event(Box::new(move |e| {
            if matches!(e, PlayerEvent::StateChanged(PlayerState::Playing)) {
                counter.set(counter.get() + 1);
            }
        }));

        let fade = Transition::Crossfade(Duration::from_secs(3));
        assert_eq!(play_next(&player, "a", fade), Transition::Cut);
        assert_eq!(player.uri.borrow().as_deref(), Some("a"));
        assert_eq!(seen.get(), 1);
        assert_eq!(
            play_next(&player, "b", Transition::Gapless),
            Transition::Cut
        );

        let fading = FakePlayer {
            crossfade: true,
            ..Default::default()
        };
        assert_eq!(play_next(&fading, "c", fade), fade);
        assert_eq!(fading.uri.borrow().as_deref(), Some("crossfade:c"));
    }
}
