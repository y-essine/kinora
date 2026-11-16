// SPDX-License-Identifier: GPL-3.0-or-later

//! Kinora: the sound of a record moved by hand (see `docs/architecture.md`,
//! "The deck's audio").
//!
//! A player cannot do this: every change of speed or direction is a seek,
//! a tenth of a second of silence at best, and backwards takes a second to
//! start. So while the record is held, the track's player rests and this
//! plays instead: a few seconds of the track's PCM around the needle, read at
//! whatever speed the hand moves, forwards or backwards, through a small
//! pipeline of its own (`appsrc` to the sound device, a few tens of
//! milliseconds from hand to ear).
//!
//! The audio comes from two places: what the player just decoded (its
//! [`Tap`](crate::gui::deck_audio::Tap), there at once) and, around that, a
//! stretch decoded in the background ([`Scratch::extend`]).
//!
//! The needle follows the hand like a platter under a finger: it heads for
//! where the hand is, at the hand's speed, with a few milliseconds of
//! inertia, so the pitch glides instead of stepping from one pointer event to
//! the next. Let go, it runs up to normal speed the way the record on the
//! screen does. Where the needle stands still there is no sound, and what is
//! read faster than recorded is dulled first, as a real stylus does.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gst::prelude::*;

use crate::gui::deck_audio::Pcm;

/// The needle reaches the hand in about this long, in seconds.
const FOLLOW: f64 = 0.035;
/// ...and its speed settles in about this long.
const INERTIA: f64 = 0.004;
/// A hand that has not moved for this long is holding still, in seconds.
const HAND_STILL: f64 = 0.07;
/// No faster than this many times the recorded speed.
const FASTEST: f64 = 12.0;
/// Let go, the record takes up speed (or stops) over about this long.
pub const SPIN_UP: f64 = 0.45;
const SPIN_DOWN: f64 = 0.25;
/// Below this speed the needle is silent.
const QUIET_BELOW: f64 = 0.06;
/// One buffer of sound, in seconds.
const BLOCK: f64 = 0.005;

/// Not grabbed yet: silent, the needle where it is.
const IDLE: u8 = 0;
const HELD: u8 = 1;
const SPINNING_UP: u8 = 2;
const STOPPING: u8 = 3;
const ENDING: u8 = 4;

fn load(cell: &AtomicU64) -> f64 {
    f64::from_bits(cell.load(Ordering::Relaxed))
}

fn store(cell: &AtomicU64, value: f64) {
    cell.store(value.to_bits(), Ordering::Relaxed);
}

/// Where the hand has the record.
struct Hand {
    position: f64,
    /// Seconds of the track per second.
    speed: f64,
    at: Instant,
}

struct Shared {
    audio: Mutex<Pcm>,
    hand: Mutex<Hand>,
    /// Put the needle here, at this speed (see [`Scratch::grab`]).
    jump: Mutex<Option<(f64, f64)>>,
    mode: AtomicU8,
    volume: AtomicU32,
    /// Where the needle is and how fast (what was last put out).
    position: AtomicU64,
    rate: AtomicU64,
    /// The needle's place for the last buffers, by their time on the
    /// pipeline's clock (nanoseconds): what is heard now is one of these.
    trail: Mutex<VecDeque<(u64, f64)>>,
    /// The needle ran into the end of the audio that is there.
    starved: AtomicBool,
    /// What was put out, kept for a test to look at (development builds).
    dump: Option<Mutex<Vec<f32>>>,
}

/// The needle: where it is in the audio and how it moves.
struct Needle {
    position: f64,
    rate: f64,
    /// The envelope that fades the sound in and out.
    level: f64,
    /// The dulling filter's memory, left and right.
    dull: [f64; 2],
    frames_out: u64,
}

/// See the module's documentation.
pub struct Scratch {
    pipeline: gst::Pipeline,
    shared: Arc<Shared>,
}

/// The value between `b` and `c` at `t`, on the curve through all four.
fn hermite(a: f64, b: f64, c: f64, d: f64, t: f64) -> f64 {
    let c0 = b;
    let c1 = 0.5 * (c - a);
    let c2 = a - 2.5 * b + 2.0 * c - 0.5 * d;
    let c3 = 0.5 * (d - a) + 1.5 * (b - c);
    ((c3 * t + c2) * t + c1) * t + c0
}

impl Needle {
    /// The next `frames` frames of sound, interleaved stereo.
    fn play(&mut self, shared: &Shared, frames: usize, out: &mut Vec<f32>) {
        if let Some((position, rate)) = shared.jump.lock().unwrap().take() {
            (self.position, self.rate, self.dull) = (position, rate, [0.0; 2]);
        }
        let mode = shared.mode.load(Ordering::Relaxed);
        self.frames_out += frames as u64;
        if mode == IDLE {
            out.resize(out.len() + frames * 2, 0.0);
            return;
        }
        let audio = shared.audio.lock().unwrap();
        let rate = audio.rate.max(1) as f64;
        let total = audio.frames();
        let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed)) as f64;
        let (mut target, hand_speed) = {
            let hand = shared.hand.lock().unwrap();
            let since = hand.at.elapsed().as_secs_f64();
            // Between two pointer events the hand goes on as it went.
            let speed = if since > HAND_STILL { 0.0 } else { hand.speed };
            (hand.position + speed * since.min(HAND_STILL), speed)
        };
        let step = 1.0 / rate;
        let inertia = 1.0 - (-step / INERTIA).exp();
        let spin_up = 1.0 - (-step / SPIN_UP).exp();
        let spin_down = 1.0 - (-step / SPIN_DOWN).exp();
        // In at once, out a little slower (the player fades in under it).
        let (fade_in, fade_out) = (step / 0.012, step / 0.05);
        let (first, last) = (audio.start + 2.0 * step, audio.end() - 3.0 * step);
        for _ in 0..frames {
            match mode {
                HELD => {
                    let want =
                        (hand_speed + (target - self.position) / FOLLOW).clamp(-FASTEST, FASTEST);
                    self.rate += (want - self.rate) * inertia;
                    target += hand_speed * step;
                }
                SPINNING_UP => self.rate += (1.0 - self.rate) * spin_up,
                STOPPING => self.rate -= self.rate * spin_down,
                // (Fading out: on as it went.)
                _ => {}
            }
            self.position += self.rate * step;
            // The audio's edges: the needle stops there.
            if total < 8 || self.position < first {
                self.position = self.position.max(first);
                self.rate = self.rate.max(0.0);
            }
            if self.position > last {
                self.position = last.max(first);
                self.rate = self.rate.min(0.0);
                shared.starved.store(true, Ordering::Relaxed);
            }
            let speed = self.rate.abs();
            let wanted_level = if mode == ENDING { 0.0 } else { 1.0 };
            self.level += (wanted_level - self.level).clamp(-fade_out, fade_in);
            // Silent where it stands still (a held sample is only a click).
            let moving = (speed / QUIET_BELOW).min(1.0);
            let gain = volume * self.level * moving * moving * (3.0 - 2.0 * moving);
            // Read faster than recorded, the top goes: dull it first.
            let open = if speed > 1.1 {
                1.0 - (-std::f64::consts::TAU * (15000.0 / speed) / rate).exp()
            } else {
                1.0
            };
            if total < 8 {
                out.extend([0.0, 0.0]);
                continue;
            }
            let at = (self.position - audio.start) * rate;
            let index = (at.floor() as usize).clamp(1, total - 3);
            let t = (at - index as f64).clamp(0.0, 1.0);
            for channel in 0..2 {
                let sample = |frame: usize| audio.samples[frame * 2 + channel] as f64;
                let value = hermite(
                    sample(index - 1),
                    sample(index),
                    sample(index + 1),
                    sample(index + 2),
                    t,
                );
                self.dull[channel] += open * (value - self.dull[channel]);
                out.push((self.dull[channel] * gain) as f32);
            }
        }
        store(&shared.position, self.position);
        store(&shared.rate, self.rate);
    }
}

impl Scratch {
    /// Get ready to play `audio` (stereo) at `volume` (linear): the pipeline
    /// runs, silent, until the record is grabbed ([`Self::grab`]).
    pub fn new(audio: Pcm, volume: f64) -> Option<Self> {
        let (position, rate) = (audio.start, 0.0f64);
        if audio.channels != 2 || audio.rate == 0 || audio.frames() < 64 {
            return None;
        }
        let sample_rate = audio.rate;
        let pipeline = gst::parse::launch(
            "appsrc name=needle format=time max-bytes=4096 ! audioconvert ! audioresample \
             ! autoaudiosink",
        )
        .inspect_err(|e| log::warn!("Scratch: no pipeline: {e}"))
        .ok()?
        .downcast::<gst::Pipeline>()
        .ok()?;
        let source = pipeline.by_name("needle")?;
        source.set_property(
            "caps",
            gst::Caps::builder("audio/x-raw")
                .field("format", "F32LE")
                .field("layout", "interleaved")
                .field("rate", sample_rate as i32)
                .field("channels", 2i32)
                .build(),
        );
        // The device, when it is made: as little between the needle and the
        // ear as it takes, and the volume said outright. A sound server gives
        // a new stream the volume it remembers for the app (its last stream's,
        // whatever that was); where the device has a volume of its own, that
        // is the one to set, and the samples are left alone.
        let device_volume = Arc::new(AtomicBool::new(false));
        pipeline.connect_deep_element_added(glib::clone!(
            #[strong]
            device_volume,
            move |_, _, element| {
                for (property, micros) in [("buffer-time", 40_000i64), ("latency-time", 10_000i64)]
                {
                    if element.find_property(property).is_some() {
                        element.set_property(property, micros);
                    }
                }
                let is = |property: &str, value: glib::Type| {
                    element
                        .find_property(property)
                        .is_some_and(|p| p.value_type() == value)
                };
                if is("volume", glib::Type::F64) && is("mute", glib::Type::BOOL) {
                    element.set_property("volume", volume);
                    element.set_property("mute", false);
                    device_volume.store(true, Ordering::Relaxed);
                }
            }
        ));

        let shared = Arc::new(Shared {
            audio: Mutex::new(audio),
            hand: Mutex::new(Hand {
                position,
                speed: 0.0,
                at: Instant::now(),
            }),
            jump: Mutex::default(),
            mode: AtomicU8::new(IDLE),
            volume: AtomicU32::new((volume as f32).to_bits()),
            position: AtomicU64::new(position.to_bits()),
            rate: AtomicU64::new(rate.to_bits()),
            trail: Mutex::default(),
            starved: AtomicBool::new(false),
            dump: (cfg!(feature = "dev-hooks") && std::env::var("KH_SCRATCH_DUMP").is_ok())
                .then(Mutex::default),
        });
        let needle = Mutex::new(Needle {
            position,
            rate,
            level: 0.0,
            dull: [0.0; 2],
            frames_out: 0,
        });
        let frames = (BLOCK * sample_rate as f64) as usize;
        // (The device is made when the pipeline is: before the first sound.)
        pipeline
            .set_state(gst::State::Ready)
            .inspect_err(|e| log::warn!("Scratch: no sound device: {e}"))
            .ok()?;
        if device_volume.load(Ordering::Relaxed) {
            shared.volume.store(1.0f32.to_bits(), Ordering::Relaxed);
        }
        source.connect(
            "need-data",
            false,
            glib::clone!(
                #[strong]
                shared,
                move |args| {
                    let source = args[0].get::<gst::Element>().ok()?;
                    let mut needle = needle.lock().unwrap();
                    let pts = needle.frames_out * 1_000_000_000 / sample_rate as u64;
                    {
                        let mut trail = shared.trail.lock().unwrap();
                        trail.push_back((pts, needle.position));
                        if trail.len() > 256 {
                            trail.pop_front();
                        }
                    }
                    let mut samples = Vec::with_capacity(frames * 2);
                    needle.play(&shared, frames, &mut samples);
                    if let Some(dump) = &shared.dump {
                        dump.lock().unwrap().extend_from_slice(&samples);
                    }
                    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                    let mut buffer = gst::Buffer::from_mut_slice(bytes);
                    if let Some(buffer) = buffer.get_mut() {
                        buffer.set_pts(gst::ClockTime::from_nseconds(pts));
                        buffer.set_duration(gst::ClockTime::from_nseconds(
                            frames as u64 * 1_000_000_000 / sample_rate as u64,
                        ));
                    }
                    let _ = source.emit_by_name::<gst::FlowReturn>("push-buffer", &[&buffer]);
                    None
                }
            ),
        );
        pipeline
            .set_state(gst::State::Playing)
            .inspect_err(|e| log::warn!("Scratch: the pipeline does not start: {e}"))
            .ok()?;
        Some(Self { pipeline, shared })
    }

    /// The record is grabbed with the needle at `position`, turning at `rate`
    /// (1: it was playing). From here on the needle follows the hand; a hand
    /// that does not move brings it to rest.
    pub fn grab(&self, position: f64, rate: f64) {
        *self.shared.hand.lock().unwrap() = Hand {
            position,
            speed: 0.0,
            at: Instant::now(),
        };
        store(&self.shared.position, position);
        self.shared.trail.lock().unwrap().clear();
        *self.shared.jump.lock().unwrap() = Some((position, rate));
        self.shared.mode.store(HELD, Ordering::Relaxed);
    }

    /// The hand has the record at `position` (seconds of the track).
    pub fn hand(&self, position: f64) {
        let mut hand = self.shared.hand.lock().unwrap();
        let now = Instant::now();
        let since = now.duration_since(hand.at).as_secs_f64();
        if since > 0.0005 {
            // (Smoothed: pointer events come in uneven steps.)
            let speed = (position - hand.position) / since;
            hand.speed = if since > HAND_STILL {
                speed
            } else {
                0.5 * hand.speed + 0.5 * speed
            };
            hand.at = now;
        }
        hand.position = position;
        self.shared.mode.store(HELD, Ordering::Relaxed);
    }

    /// The hand let go: the record takes up normal speed (`play`), or comes
    /// to rest.
    pub fn release(&self, play: bool) {
        self.shared
            .mode
            .store(if play { SPINNING_UP } else { STOPPING }, Ordering::Relaxed);
    }

    /// Fade out (a hundredth of a second); the object can be dropped a
    /// moment later.
    pub fn end(&self) {
        self.shared.mode.store(ENDING, Ordering::Relaxed);
    }

    /// The needle's speed, 1 being the recorded one.
    pub fn rate(&self) -> f64 {
        load(&self.shared.rate)
    }

    /// Where the needle is heard right now, in seconds of the track (what
    /// was put out last is heard a few hundredths of a second later).
    pub fn position(&self) -> f64 {
        let now = self
            .pipeline
            .current_running_time()
            .map(|time| time.nseconds());
        let trail = self.shared.trail.lock().unwrap();
        now.and_then(|now| {
            let after = trail.partition_point(|(pts, _)| *pts <= now);
            let (pts, position) = *trail.get(after.checked_sub(1)?)?;
            // Within a buffer: on at the speed of the next one.
            let next = trail.get(after).map_or(position, |(_, next)| *next);
            let part = ((now - pts) as f64 / (BLOCK * 1e9)).min(1.0);
            Some(position + (next - position) * part)
        })
        .unwrap_or_else(|| load(&self.shared.position))
    }

    /// The audio that is there, from where to where in the track.
    pub fn range(&self) -> (f64, f64) {
        let audio = self.shared.audio.lock().unwrap();
        (audio.start, audio.end())
    }

    pub fn sample_rate(&self) -> u32 {
        self.shared.audio.lock().unwrap().rate
    }

    /// The needle reached the end of the audio that is there.
    pub fn starved(&self) -> bool {
        self.shared.starved.load(Ordering::Relaxed)
    }

    /// A handle to add audio with from another thread.
    pub fn feeder(&self) -> Feeder {
        Feeder(Arc::downgrade(&self.shared))
    }

    /// What was put out so far (development builds, with `KH_SCRATCH_DUMP`).
    #[cfg(feature = "dev-hooks")]
    pub fn dump(&self) -> Option<(Vec<f32>, u32)> {
        let rate = self.shared.audio.lock().unwrap().rate;
        self.shared
            .dump
            .as_ref()
            .map(|dump| (dump.lock().unwrap().clone(), rate))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Adds audio to a [`Scratch`] (if it is still there).
#[derive(Clone)]
pub struct Feeder(std::sync::Weak<Shared>);

impl Feeder {
    /// Whether the scratch is still there to take audio.
    pub fn wanted(&self) -> bool {
        self.0.strong_count() > 0
    }

    /// More of the track: `more` (stereo, the same rate) is joined on where
    /// it continues what is there, before or after; what overlaps is
    /// dropped. `false` when it does not join on (or nobody is listening).
    pub fn extend(&self, more: &Pcm) -> bool {
        let Some(shared) = self.0.upgrade() else {
            return false;
        };
        let mut audio = shared.audio.lock().unwrap();
        if more.rate != audio.rate || more.channels != 2 || more.samples.is_empty() {
            return false;
        }
        let rate = audio.rate as f64;
        // (Half a frame of slack: the times are rounded.)
        let slack = 0.5 / rate;
        let joined = if more.start <= audio.end() + slack && more.end() > audio.end() {
            // It goes on after the end.
            let skip =
                (((audio.end() - more.start) * rate).round().max(0.0) as usize).min(more.frames());
            audio.samples.extend_from_slice(&more.samples[skip * 2..]);
            shared.starved.store(false, Ordering::Relaxed);
            true
        } else if more.end() >= audio.start - slack && more.start < audio.start {
            // It leads up to the start.
            let keep =
                (((audio.start - more.start) * rate).round().max(0.0) as usize).min(more.frames());
            let mut samples = Vec::with_capacity(keep * 2 + audio.samples.len());
            samples.extend_from_slice(&more.samples[..keep * 2]);
            samples.extend_from_slice(&audio.samples);
            audio.samples = samples;
            audio.start -= keep as f64 / rate;
            true
        } else {
            false
        };
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(audio: Pcm, position: f64) -> Shared {
        Shared {
            audio: Mutex::new(audio),
            hand: Mutex::new(Hand {
                position,
                speed: 0.0,
                at: Instant::now(),
            }),
            jump: Mutex::default(),
            mode: AtomicU8::new(HELD),
            volume: AtomicU32::new(1.0f32.to_bits()),
            position: AtomicU64::new(position.to_bits()),
            rate: AtomicU64::new(0),
            trail: Mutex::default(),
            starved: AtomicBool::new(false),
            dump: None,
        }
    }

    /// Ten seconds of a 440 Hz tone that starts 100 s into its track.
    fn tone() -> Pcm {
        let rate = 44100;
        Pcm {
            samples: (0..rate * 10)
                .flat_map(|i| {
                    let v = (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin() * 0.5;
                    [v, v]
                })
                .collect(),
            rate,
            channels: 2,
            start: 100.0,
        }
    }

    fn needle(position: f64, rate: f64) -> Needle {
        Needle {
            position,
            rate,
            level: 1.0,
            dull: [0.0; 2],
            frames_out: 0,
        }
    }

    #[test]
    fn let_go_the_needle_runs_up_to_the_recorded_speed() {
        let shared = shared(tone(), 104.0);
        shared.mode.store(SPINNING_UP, Ordering::Relaxed);
        let mut needle = needle(104.0, -2.0);
        let mut out = Vec::new();
        // Two seconds: backwards at first, then forwards, then as recorded.
        needle.play(&shared, 44100 / 10, &mut out);
        assert!(needle.rate < 0.0 && needle.position < 104.0);
        needle.play(&shared, 44100 * 3, &mut out);
        assert!((needle.rate - 1.0).abs() < 0.02, "{}", needle.rate);
        // At the recorded speed the tone comes out as it went in.
        let before = out.len();
        needle.play(&shared, 4410, &mut out);
        let crossings = out[before..]
            .chunks_exact(2)
            .map(|frame| frame[0])
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count();
        assert!(
            (43..=45).contains(&crossings),
            "{crossings} cycles in 0.1 s"
        );
        assert!(out.iter().all(|v| v.abs() <= 0.52));
    }

    #[test]
    fn held_still_the_needle_is_silent_and_stays_inside_the_audio() {
        let shared = shared(tone(), 105.0);
        let mut needle = needle(105.0, 1.0);
        let mut out = Vec::new();
        // The hand does not move: the record brakes, the sound goes.
        std::thread::sleep(std::time::Duration::from_millis(80));
        needle.play(&shared, 44100 / 2, &mut out);
        assert!(needle.rate.abs() < 0.01 && (needle.position - 105.0).abs() < 0.01);
        assert!(out[out.len() - 200..].iter().all(|v| v.abs() < 1e-3));

        // A hand far outside what is there: the needle stops at the edge.
        shared.hand.lock().unwrap().position = 50.0;
        needle.play(&shared, 44100 * 2, &mut out);
        assert!(needle.position >= 100.0 && needle.position < 100.01);
        shared.hand.lock().unwrap().position = 500.0;
        needle.play(&shared, 44100 * 2, &mut out);
        assert!(needle.position <= 110.0 && needle.position > 109.99);
        assert!(shared.starved.load(Ordering::Relaxed));
        assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 0.75));
    }

    #[test]
    fn audio_joins_on_at_either_end() {
        let shared = Arc::new(shared(tone(), 105.0));
        let feeder = Feeder(Arc::downgrade(&shared));
        let piece = |start: f64, seconds: f64| Pcm {
            samples: vec![0.25; (seconds * 44100.0) as usize * 2],
            rate: 44100,
            channels: 2,
            start,
        };
        // After the end, overlapping it by half a second.
        assert!(feeder.extend(&piece(109.5, 2.0)));
        let range = |shared: &Shared| {
            let audio = shared.audio.lock().unwrap();
            (audio.start, audio.end())
        };
        let (start, end) = range(&shared);
        assert!((start - 100.0).abs() < 1e-9 && (end - 111.5).abs() < 1e-4);
        // Before the start.
        assert!(feeder.extend(&piece(97.0, 3.2)));
        let (start, end) = range(&shared);
        assert!((start - 97.0).abs() < 1e-4 && (end - 111.5).abs() < 1e-4);
        // What does not touch is left out; so is another rate.
        assert!(!feeder.extend(&piece(120.0, 1.0)));
        assert!(!feeder.extend(&piece(90.0, 1.0)));
        assert!(!feeder.extend(&Pcm {
            rate: 48000,
            ..piece(111.0, 1.0)
        }));
        // The tone is still where it was.
        let audio = shared.audio.lock().unwrap();
        let frame = ((100.0 - audio.start) * 44100.0).round() as usize + 25;
        let expected = (std::f32::consts::TAU * 440.0 * 25.0 / 44100.0).sin() * 0.5;
        assert!((audio.samples[frame * 2] - expected).abs() < 1e-4);
    }
}
