//! Planning the move from one track to the next: when the next one starts
//! and how the two volumes cross. Pure arithmetic: the frontend's player
//! applies the volumes (two players, or one with two streams).

use std::f64::consts::FRAC_PI_2;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::player::Transition;

/// The crossfade when the setting is not changed.
pub const DEFAULT_CROSSFADE: Duration = Duration::from_secs(6);
/// The longest crossfade the setting allows.
pub const MAX_CROSSFADE: Duration = Duration::from_secs(12);
/// Tracks shorter than this get no crossfade at all (jingles, intros).
pub const MIN_TRACK_FOR_FADE: Duration = Duration::from_secs(30);
/// A crossfade takes at most this share of the shorter track (a 40 s track
/// fades for 5 s at most), so short tracks get short fades.
pub const MAX_FADE_SHARE: f64 = 1.0 / 8.0;
/// Fades shorter than this are not worth it: a cut instead.
pub const MIN_FADE: Duration = Duration::from_millis(500);

/// How the current track gives way to the next one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TransitionPlan {
    /// Where in the current track the next one starts. `None`: when the
    /// current one ends (its end of stream), for a cut or an unknown length.
    pub start_at: Option<Duration>,
    /// How long both play while one fades out and the other in.
    /// [`Duration::ZERO`]: a cut.
    pub fade: Duration,
}

/// The volumes of both tracks at one moment of a crossfade, as linear
/// amplitudes in `0.0..=1.0` (what GStreamer's and mpv's volume properties
/// take, mpv as a percentage).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FadeGains {
    /// The track that is ending.
    pub outgoing: f64,
    /// The track that is starting.
    pub incoming: f64,
}

/// One point of a crossfade: at `offset` after the fade began, set these
/// volumes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FadePoint {
    pub offset: Duration,
    pub gains: FadeGains,
}

impl TransitionPlan {
    /// A plain cut at the end of the current track.
    pub const CUT: Self = Self {
        start_at: None,
        fade: Duration::ZERO,
    };

    pub fn is_cut(&self) -> bool {
        self.fade.is_zero()
    }

    /// The same plan as a [`Transition`], for [`crate::player::play_next`]
    /// with players that crossfade by themselves.
    pub fn transition(&self) -> Transition {
        if self.is_cut() {
            Transition::Cut
        } else {
            Transition::Crossfade(self.fade)
        }
    }

    /// Whether the next track should be started now, with the current one
    /// at `position`. Always `false` for a cut (start it on end of stream).
    pub fn should_start(&self, position: Duration) -> bool {
        self.start_at.is_some_and(|at| position >= at)
    }

    /// How long until the next track starts, from `position`; `None` for a
    /// cut. Handy for one timer instead of polling the position.
    pub fn time_until_start(&self, position: Duration) -> Option<Duration> {
        self.start_at.map(|at| at.saturating_sub(position))
    }

    /// The volumes `elapsed` after the next track started. Before the fade:
    /// only the outgoing track; after it: only the incoming one.
    pub fn gains(&self, elapsed: Duration) -> FadeGains {
        if self.is_cut() {
            return FadeGains {
                outgoing: 0.0,
                incoming: 1.0,
            };
        }
        equal_power(elapsed.as_secs_f64() / self.fade.as_secs_f64())
    }

    /// The volumes when the current track is at `position` (before the start
    /// point: only the current track).
    pub fn gains_at_position(&self, position: Duration) -> FadeGains {
        match self.start_at {
            Some(at) if position >= at => self.gains(position - at),
            _ => equal_power(0.0),
        }
    }

    /// The curve as `steps + 1` points from the start of the fade to its
    /// end, for players that ramp volumes from a list (or a timer that sets
    /// them every `fade / steps`). A cut is the single point "incoming at
    /// full volume".
    pub fn curve(&self, steps: usize) -> Vec<FadePoint> {
        if self.is_cut() || steps == 0 {
            return vec![FadePoint {
                offset: Duration::ZERO,
                gains: self.gains(self.fade),
            }];
        }
        (0..=steps)
            .map(|i| {
                let offset = self.fade.mul_f64(i as f64 / steps as f64);
                FadePoint {
                    offset,
                    gains: self.gains(offset),
                }
            })
            .collect()
    }
}

/// The equal-power crossfade at `progress` (0.0 = start, 1.0 = done,
/// clamped): `outgoing = cos(p·π/2)`, `incoming = sin(p·π/2)`, so the sum of
/// their powers stays 1 and the loudness does not dip in the middle (a linear
/// fade sounds quieter halfway).
pub fn equal_power(progress: f64) -> FadeGains {
    let p = if progress.is_nan() {
        0.0
    } else {
        progress.clamp(0.0, 1.0)
    };
    FadeGains {
        outgoing: (p * FRAC_PI_2).cos(),
        incoming: (p * FRAC_PI_2).sin(),
    }
}

/// Plan the move from the current track (`duration`, now at `position`) to
/// the next one (`next_duration`), with `crossfade` as the wanted length
/// (the setting, clamped to [`MAX_CROSSFADE`]).
///
/// - No fade (a cut on end of stream) when the setting is 0, when either
///   length is unknown (live streams), or when either track is shorter than
///   [`MIN_TRACK_FOR_FADE`].
/// - The fade is at most [`MAX_FADE_SHARE`] of the shorter track.
/// - Planned late (the user seeked near the end), the fade starts now and
///   only lasts what is left; under [`MIN_FADE`] left, a cut.
///
/// Call it again after a seek: the plan depends on the position only when it
/// is already past the start point.
pub fn plan_transition(
    duration: Option<Duration>,
    position: Duration,
    next_duration: Option<Duration>,
    crossfade: Duration,
) -> TransitionPlan {
    let (Some(duration), Some(next)) = (duration, next_duration) else {
        return TransitionPlan::CUT;
    };
    let shorter = duration.min(next);
    if crossfade.is_zero() || shorter < MIN_TRACK_FOR_FADE {
        return TransitionPlan::CUT;
    }
    let mut fade = crossfade
        .min(MAX_CROSSFADE)
        .min(shorter.mul_f64(MAX_FADE_SHARE));
    let mut start = duration - fade;
    if position > start {
        start = position;
        fade = duration.saturating_sub(position);
    }
    if fade < MIN_FADE {
        return TransitionPlan::CUT;
    }
    TransitionPlan {
        start_at: Some(start),
        fade,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(u64) -> Duration = Duration::from_secs;

    fn plan(duration: u64, position: u64, next: u64, fade: u64) -> TransitionPlan {
        plan_transition(Some(S(duration)), S(position), Some(S(next)), S(fade))
    }

    #[test]
    fn ordinary_tracks_fade_for_the_setting() {
        let p = plan(200, 10, 180, 6);
        assert_eq!(p.start_at, Some(S(194)));
        assert_eq!(p.fade, S(6));
        assert_eq!(p.transition(), Transition::Crossfade(S(6)));
        assert!(!p.should_start(S(193)));
        assert!(p.should_start(S(194)));
        assert_eq!(p.time_until_start(S(190)), Some(S(4)));
        // The setting is capped at 12 s.
        assert_eq!(plan(300, 0, 300, 30).fade, MAX_CROSSFADE);
    }

    #[test]
    fn short_tracks_fade_shorter_and_very_short_ones_cut() {
        // 40 s: at most 5 s.
        assert_eq!(plan(40, 0, 200, 6).fade, S(5));
        assert_eq!(plan(200, 0, 40, 6).fade, S(5));
        assert_eq!(plan(29, 0, 200, 6), TransitionPlan::CUT);
        assert_eq!(plan(200, 0, 10, 6), TransitionPlan::CUT);
    }

    #[test]
    fn no_fade_when_off_or_unknown() {
        assert_eq!(plan(200, 0, 200, 0), TransitionPlan::CUT);
        assert!(plan(200, 0, 200, 0).is_cut());
        assert_eq!(TransitionPlan::CUT.transition(), Transition::Cut);
        assert_eq!(
            plan_transition(None, S(0), Some(S(200)), S(6)),
            TransitionPlan::CUT
        );
        assert_eq!(
            plan_transition(Some(S(200)), S(0), None, S(6)),
            TransitionPlan::CUT
        );
        assert!(!TransitionPlan::CUT.should_start(S(1000)));
        assert_eq!(TransitionPlan::CUT.time_until_start(S(0)), None);
    }

    #[test]
    fn late_plans_fade_over_what_is_left() {
        let p = plan(200, 197, 200, 6);
        assert_eq!(p.start_at, Some(S(197)));
        assert_eq!(p.fade, S(3));
        // Too little left: a cut.
        let p = plan_transition(
            Some(S(200)),
            Duration::from_millis(199_700),
            Some(S(200)),
            S(6),
        );
        assert_eq!(p, TransitionPlan::CUT);
    }

    #[test]
    fn the_curve_is_equal_power() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let start = equal_power(0.0);
        assert!(close(start.outgoing, 1.0) && close(start.incoming, 0.0));
        let end = equal_power(1.0);
        assert!(close(end.outgoing, 0.0) && close(end.incoming, 1.0));
        let mid = equal_power(0.5);
        assert!(close(mid.outgoing, mid.incoming));
        assert!(close(mid.outgoing, std::f64::consts::FRAC_1_SQRT_2));
        // Constant power everywhere, and clamped outside 0..1.
        for i in 0..=20 {
            let g = equal_power(i as f64 / 20.0);
            assert!(close(g.outgoing.powi(2) + g.incoming.powi(2), 1.0));
        }
        assert_eq!(equal_power(-1.0), start);
        assert_eq!(equal_power(2.0), equal_power(1.0));
        assert_eq!(equal_power(f64::NAN), start);

        let p = plan(200, 0, 200, 6);
        let curve = p.curve(6);
        assert_eq!(curve.len(), 7);
        assert_eq!(curve[0].offset, S(0));
        assert_eq!(curve[6].offset, S(6));
        assert!(close(curve[3].gains.outgoing, mid.outgoing));
        assert!(
            curve
                .windows(2)
                .all(|w| w[0].gains.outgoing >= w[1].gains.outgoing
                    && w[0].gains.incoming <= w[1].gains.incoming)
        );
        // By position in the current track.
        assert_eq!(p.gains_at_position(S(100)), start);
        assert!(close(p.gains_at_position(S(197)).incoming, mid.incoming));
        assert_eq!(p.gains_at_position(S(250)), equal_power(1.0));

        assert_eq!(TransitionPlan::CUT.curve(10).len(), 1);
        assert_eq!(TransitionPlan::CUT.gains(S(0)).incoming, 1.0);
    }
}
