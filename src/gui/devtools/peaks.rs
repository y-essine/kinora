// SPDX-License-Identifier: GPL-3.0-or-later

//! The Dev panel's waveforms: a track's audio reduced to [`PER_SECOND`]
//! bins, each with its peak and the level of three bands (kick, body, air),
//! worked out once as the audio comes by and then only drawn.
//!
//! The bins come from two places: a background decode of the whole track
//! (from its prepared stream, while the panel shows its deck), and what the
//! deck's `Tap` kept of the audio that played (which fills what the decode
//! has not reached, or cannot: a track without a prepared stream).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Bins per second of the track.
pub const PER_SECOND: f64 = 50.0;
/// The bands' edges, in Hz.
const LOW_BELOW: f64 = 180.0;
const HIGH_ABOVE: f64 = 2200.0;
/// How many tracks are kept (a 5 minute track is 75 kB).
const TRACKS: usize = 10;
/// The longest track that is decoded in full, in seconds.
const LONGEST: f64 = 1500.0;

/// One bin: 0 to 255 each. `amp` is the peak, the others band levels.
#[derive(Clone, Copy, Default)]
pub struct Bin {
    pub amp: u8,
    pub low: u8,
    pub mid: u8,
    pub high: u8,
    pub known: bool,
}

/// Turns a run of samples into bins.
#[derive(Default)]
pub struct Reader {
    /// The time after the last frame, to notice a gap.
    end: f64,
    low: f64,
    body: f64,
    bin: usize,
    frames: u32,
    peak: f32,
    sums: [f64; 3],
}

impl Reader {
    /// `samples`: interleaved, `channels` of them, at `rate`, from `start`
    /// seconds into the track. Every finished bin goes to `put`.
    pub fn feed(
        &mut self,
        samples: &[f32],
        channels: usize,
        rate: u32,
        start: f64,
        put: &mut impl FnMut(usize, Bin),
    ) {
        if channels == 0 || rate == 0 {
            return;
        }
        if (start - self.end).abs() > 0.01 {
            *self = Self::default();
            self.bin = (start * PER_SECOND) as usize;
        }
        let rate = rate as f64;
        let k_low = 1.0 - (-std::f64::consts::TAU * LOW_BELOW / rate).exp();
        let k_body = 1.0 - (-std::f64::consts::TAU * HIGH_ABOVE / rate).exp();
        for (index, frame) in samples.chunks_exact(channels).enumerate() {
            let bin = ((start + index as f64 / rate) * PER_SECOND) as usize;
            if bin != self.bin {
                self.flush(put);
                self.bin = bin;
            }
            let x = if channels == 1 {
                frame[0]
            } else {
                (frame[0] + frame[1]) * 0.5
            };
            let wide = x as f64;
            self.low += k_low * (wide - self.low);
            self.body += k_body * (wide - self.body);
            let (low, mid, high) = (self.low, self.body - self.low, wide - self.body);
            self.sums[0] += low * low;
            self.sums[1] += mid * mid;
            self.sums[2] += high * high;
            self.peak = self.peak.max(x.abs());
            self.frames += 1;
        }
        self.end = start + (samples.len() / channels) as f64 / rate;
    }

    fn flush(&mut self, put: &mut impl FnMut(usize, Bin)) {
        if self.frames == 0 {
            return;
        }
        // RMS of a band to a height: louder music does not just clip, and
        // the quiet highs still show.
        let level = |sum: f64, gain: f64| {
            let rms = (sum / self.frames as f64).sqrt();
            ((rms * gain).powf(0.6) * 255.0).min(255.0) as u8
        };
        put(
            self.bin,
            Bin {
                amp: (self.peak.min(1.0) * 255.0) as u8,
                low: level(self.sums[0], 2.2),
                mid: level(self.sums[1], 2.0),
                high: level(self.sums[2], 2.6),
                known: true,
            },
        );
        (self.frames, self.peak, self.sums) = (0, 0.0, [0.0; 3]);
    }
}

/// Where a track's whole-track decode is.
#[derive(Clone, PartialEq, Default)]
pub enum Decode {
    #[default]
    No,
    Running,
    Done,
    Failed(String),
}

impl Decode {
    pub fn label(&self) -> String {
        match self {
            Self::No => "live only".into(),
            Self::Running => "decoding".into(),
            Self::Done => "whole track".into(),
            Self::Failed(why) => format!("decode failed: {why}"),
        }
    }
}

pub struct Track {
    pub bins: Vec<Bin>,
    pub decode: Decode,
    /// How many bins are known.
    pub known: usize,
    live: Reader,
    /// The track time up to which the tap's audio was read.
    pub live_end: f64,
    touched: Instant,
}

impl Track {
    fn new() -> Self {
        Self {
            bins: Vec::new(),
            decode: Decode::No,
            known: 0,
            live: Reader::default(),
            live_end: f64::MIN,
            touched: Instant::now(),
        }
    }

    fn put(&mut self, index: usize, bin: Bin, replace: bool) {
        if index >= (LONGEST * 2.0 * PER_SECOND) as usize {
            return;
        }
        if index >= self.bins.len() {
            self.bins.resize(index + 1, Bin::default());
        }
        let slot = &mut self.bins[index];
        if !slot.known {
            self.known += 1;
        } else if !replace {
            return;
        }
        *slot = bin;
    }

    pub fn known_seconds(&self) -> f64 {
        self.known as f64 / PER_SECOND
    }
}

static STORE: LazyLock<Mutex<HashMap<String, Track>>> = LazyLock::new(Default::default);
/// One decode at a time, and none once the panel is closed.
static DECODING: AtomicBool = AtomicBool::new(false);
static WANTED: AtomicBool = AtomicBool::new(false);

/// The panel shows waveforms (decodes go on) or not (they stop).
pub fn set_wanted(wanted: bool) {
    WANTED.store(wanted, Ordering::Relaxed);
}

fn track<'a>(store: &'a mut HashMap<String, Track>, id: &str) -> &'a mut Track {
    if !store.contains_key(id) {
        if store.len() >= TRACKS
            && let Some(oldest) = store
                .iter()
                .filter(|(_, t)| t.decode != Decode::Running)
                .min_by_key(|(_, t)| t.touched)
                .map(|(id, _)| id.clone())
        {
            store.remove(&oldest);
        }
        store.insert(id.to_owned(), Track::new());
    }
    let track = store.get_mut(id).unwrap();
    track.touched = Instant::now();
    track
}

/// Look at a track's bins (`None`: nothing of it was seen yet).
pub fn with<R>(id: &str, read: impl FnOnce(Option<&Track>) -> R) -> R {
    read(STORE.lock().unwrap().get(id))
}

/// Audio that played (stereo, from the deck's tap): fills the bins the
/// decode has not given. `read` is handed a sink to pour the tap into, and
/// the track time from which the audio is new.
pub fn feed_live(id: &str, read: impl FnOnce(f64, &mut dyn FnMut(&[f32], u32, f64))) {
    let mut store = STORE.lock().unwrap();
    let track = track(&mut store, id);
    let mut reader = std::mem::take(&mut track.live);
    let mut end = track.live_end;
    let from = end;
    read(from, &mut |samples, rate, start| {
        reader.feed(samples, 2, rate, start, &mut |index, bin| {
            track.put(index, bin, false)
        });
        end = start + (samples.len() / 2) as f64 / rate.max(1) as f64;
    });
    track.live = reader;
    track.live_end = end;
}

/// The tap's audio is read again from wherever it is now (after a seek).
pub fn live_restart(id: &str, at: f64) {
    if let Some(track) = STORE.lock().unwrap().get_mut(id)
        && track.live_end > at + 4.0
    {
        track.live_end = f64::MIN;
    }
}

/// Decode the whole track at `uri` in the background, unless that was done,
/// is being done, or another track is being decoded.
pub fn ensure_decoded(id: &str, uri: &str, length: Option<f64>) {
    {
        let mut store = STORE.lock().unwrap();
        let track = track(&mut store, id);
        if track.decode != Decode::No || DECODING.swap(true, Ordering::Relaxed) {
            return;
        }
        track.decode = Decode::Running;
    }
    let (id, uri) = (id.to_owned(), uri.to_owned());
    let seconds = length.map_or(LONGEST, |length| (length + 2.0).min(LONGEST));
    let spawned = std::thread::Builder::new()
        .name("kinora-devwave".into())
        .spawn(move || {
            let reader = Arc::new(Mutex::new(Reader::default()));
            let started = Instant::now();
            let pieces: crate::gui::deck_audio::Pieces = Arc::new({
                let id = id.clone();
                move |pcm| {
                    let mut store = STORE.lock().unwrap();
                    let Some(track) = store.get_mut(&id) else {
                        return false;
                    };
                    reader.lock().unwrap().feed(
                        &pcm.samples,
                        pcm.channels,
                        pcm.rate,
                        pcm.start,
                        &mut |index, bin| track.put(index, bin, true),
                    );
                    WANTED.load(Ordering::Relaxed)
                }
            });
            let result = crate::gui::deck_audio::decode(
                &uri,
                0.0,
                seconds,
                kinora_core::beats::ANALYSIS_RATE,
                1,
                Duration::from_secs(180),
                Some(pieces),
            );
            let stopped = !WANTED.load(Ordering::Relaxed);
            let mut store = STORE.lock().unwrap();
            if let Some(track) = store.get_mut(&id) {
                track.decode = match result {
                    // Closed half way: it is asked for again next time.
                    _ if stopped => Decode::No,
                    Ok(_) => Decode::Done,
                    Err(_) if track.known_seconds() > seconds - 5.0 => Decode::Done,
                    Err(why) => Decode::Failed(why),
                };
                log::debug!(
                    "DEV: waveform of {id}: {} ({:.0} s in {:.1} s)",
                    track.decode.label(),
                    track.known_seconds(),
                    started.elapsed().as_secs_f64()
                );
            }
            DECODING.store(false, Ordering::Relaxed);
        });
    if spawned.is_err() {
        DECODING.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_lands_in_its_band() {
        let rate = 11025u32;
        let tone = |hz: f64| -> Vec<f32> {
            (0..rate)
                .map(|n| (0.5 * (std::f64::consts::TAU * hz * n as f64 / rate as f64).sin()) as f32)
                .collect()
        };
        let read = |samples: &[f32]| {
            let (mut reader, mut bins) = (Reader::default(), Vec::new());
            reader.feed(samples, 1, rate, 0.0, &mut |index, bin| bins.push((index, bin)));
            bins
        };
        let kick = read(&tone(60.0));
        assert_eq!(kick.len(), 49, "a second is 50 bins, the last still open");
        assert_eq!(kick[10].0, 10);
        let bin = kick[30].1;
        assert!(bin.low > bin.mid && bin.mid > bin.high, "60 Hz is low");
        assert!((120..=130).contains(&bin.amp), "half scale: {}", bin.amp);
        let hat = read(&tone(4500.0))[30].1;
        assert!(hat.high > hat.mid && hat.high > hat.low, "4.5 kHz is high");
    }

    #[test]
    fn live_audio_fills_only_what_is_missing() {
        let mut track = Track::new();
        let bin = |amp| Bin {
            amp,
            known: true,
            ..Bin::default()
        };
        track.put(3, bin(10), true);
        track.put(3, bin(20), false);
        track.put(5, bin(30), false);
        assert_eq!((track.bins[3].amp, track.bins[5].amp, track.known), (10, 30, 2));
        assert!(!track.bins[4].known);
    }
}
