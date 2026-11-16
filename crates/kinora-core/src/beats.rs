//! Beat analysis for the DJ's beat-matched crossfades: the tempo and the
//! beat grid of a piece of audio, how far one track has to be stretched to
//! match another, and the results kept on disk by video id.
//!
//! The core decodes nothing: a frontend hands in PCM (mono, a low sample rate
//! is enough: 11 kHz) from whatever plays its audio, and gets a [`BeatGrid`]
//! back. The DJ analyses a track's first [`INTRO`] seconds ahead of time (the
//! part a crossfade brings in), and the last seconds of what plays when the
//! fade starts.
//!
//! The method is the classic cheap one:
//!
//! 1. an **onset envelope** at about 200 Hz: the energy of three bands (kick,
//!    body, hats), compressed, its rise from one step to the next, and what
//!    stands out from the local mean ([`onset_envelope`]);
//! 2. the **tempo**: the envelope's autocorrelation, read with a comb (a beat
//!    period also shows at its multiples) and a broad preference for tempos
//!    around 120 BPM, which settles most half/double doubts; then refined
//!    against the envelope itself, where the beats of the whole window have
//!    to stay in step;
//! 3. the **phase**: the offset at which a pulse train of that period
//!    collects the most onset energy.
//!
//! Half and double tempo stay ambiguous for some music (a 70 BPM track may
//! come out as 140). [`match_tempo`] therefore also tries the incoming track
//! at half and at double its tempo.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The slowest and the fastest tempo the analysis reports.
pub const MIN_BPM: f64 = 60.0;
pub const MAX_BPM: f64 = 200.0;
/// How far a track may be stretched to match another: 8 % either way.
pub const MAX_STRETCH: f64 = 0.08;
/// How much of a track's start is analysed ahead of time, in seconds.
pub const INTRO: f64 = 45.0;
/// The sample rate the analysis wants (anything near it will do).
pub const ANALYSIS_RATE: u32 = 11025;
/// Below this confidence a grid is not worth matching to. (Measured on real
/// tracks: a rap verse over a steady beat gives 0.13 to 0.35 and the right
/// tempo, an intro without drums under 0.1 and any tempo.)
pub const MIN_CONFIDENCE: f64 = 0.12;
/// Bump to throw away what older versions kept on disk.
const CACHE_VERSION: u32 = 2;

/// The envelope's rate, about: one step every 5 ms.
const ENVELOPE_RATE: f64 = 200.0;
/// Tempos near this are preferred, within about an octave.
const USUAL_BPM: f64 = 120.0;
const USUAL_SPREAD_OCTAVES: f64 = 0.6;
/// With a tempo to go by, look this far around it.
const PRIOR_RANGE: f64 = 0.06;
/// Less audio than this says nothing about a tempo, in seconds.
const MIN_AUDIO: f64 = 4.0;

/// A tempo and where its beats fall.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BeatGrid {
    pub bpm: f64,
    /// The time of one beat, in seconds of the track.
    pub anchor: f64,
    /// 0 to 1: how alike the onsets are from one beat to the next. A drum
    /// machine gives 0.8 and more, music without a pulse next to nothing.
    pub confidence: f64,
}

impl BeatGrid {
    /// Seconds from one beat to the next.
    pub fn period(&self) -> f64 {
        60.0 / self.bpm
    }

    /// Where `position` (seconds of the track) lies between two beats, 0 to 1.
    pub fn phase_at(&self, position: f64) -> f64 {
        ((position - self.anchor) / self.period()).rem_euclid(1.0)
    }

    /// Clear enough to match another track to.
    pub fn is_usable(&self) -> bool {
        self.confidence >= MIN_CONFIDENCE && (MIN_BPM..=MAX_BPM).contains(&self.bpm)
    }
}

/// Mono audio at a low rate out of interleaved PCM: the channels are mixed
/// and every `rate / target_rate` frames averaged (a box filter is enough
/// for an onset envelope). Returns the samples and their rate.
pub fn downmix(
    interleaved: &[f32],
    channels: usize,
    rate: u32,
    target_rate: u32,
) -> (Vec<f32>, u32) {
    let channels = channels.max(1);
    let factor = (rate / target_rate.max(1)).max(1) as usize;
    let frames = interleaved.len() / channels;
    let mut out = Vec::with_capacity(frames / factor + 1);
    let scale = 1.0 / (factor * channels) as f32;
    for chunk in interleaved[..frames * channels].chunks_exact(factor * channels) {
        out.push(chunk.iter().sum::<f32>() * scale);
    }
    (out, rate / factor as u32)
}

/// What the envelopes are made of: for every step (about 5 ms) of mono
/// `pcm`, how much each of three bands (kick, body, hats) got louder since
/// the step before. Also the steps per second and the samples per step.
/// Empty for silence and for less than a few steps.
fn band_flux(pcm: &[f32], rate: u32) -> (Vec<[f32; 3]>, f64, usize) {
    let hop = ((rate as f64 / ENVELOPE_RATE).round() as usize).max(1);
    let envelope_rate = rate as f64 / hop as f64;
    let steps = pcm.len() / hop;
    if steps < 8 {
        return (Vec::new(), envelope_rate, hop);
    }
    let pole = |hz: f64| (1.0 - (-std::f64::consts::TAU * hz / rate as f64).exp()) as f32;
    let (low_pole, mid_pole) = (pole(180.0), pole(2500.0));
    // Each band's energy, smoothed over about 8 ms and read once per step.
    let smooth = (1.0 - (-1.0 / (0.008 * rate as f64)).exp()) as f32;
    let (mut low_a, mut low_b, mut mid_lp) = (0.0f32, 0.0f32, 0.0f32);
    let mut energy = [0.0f32; 3];
    let mut bands: Vec<[f32; 3]> = Vec::with_capacity(steps);
    for (index, &x) in pcm[..steps * hop].iter().enumerate() {
        low_a += low_pole * (x - low_a);
        low_b += low_pole * (low_a - low_b);
        mid_lp += mid_pole * (x - mid_lp);
        let parts = [low_b, mid_lp - low_b, x - mid_lp];
        for (e, part) in energy.iter_mut().zip(parts) {
            *e += smooth * (part * part - *e);
        }
        if index % hop == hop - 1 {
            bands.push(energy);
        }
    }
    let total: f32 = bands.iter().map(|b| b.iter().sum::<f32>()).sum::<f32>() / steps as f32;
    if total <= 1e-12 {
        return (Vec::new(), envelope_rate, hop);
    }
    // Loudness, not energy: a kick under a loud pad still counts.
    let loud = |e: f32| (1.0 + 20.0 * e / total).ln();
    let mut flux = vec![[0.0f32; 3]; steps];
    for i in 1..steps {
        for b in 0..3 {
            flux[i][b] = (loud(bands[i][b]) - loud(bands[i - 1][b])).max(0.0);
        }
    }
    (flux, envelope_rate, hop)
}

/// What stands out of `flux` from the mean of the 0.4 s around it.
fn stand_out(flux: &[f32], envelope_rate: f64) -> Vec<f32> {
    let steps = flux.len();
    let half = (0.2 * envelope_rate) as usize;
    let mut sums = Vec::with_capacity(steps + 1);
    sums.push(0.0f64);
    for &f in flux {
        sums.push(sums.last().unwrap() + f as f64);
    }
    (0..steps)
        .map(|i| {
            let (from, to) = (i.saturating_sub(half), (i + half + 1).min(steps));
            let mean = (sums[to] - sums[from]) / (to - from) as f64;
            (flux[i] as f64 - mean).max(0.0) as f32
        })
        .collect()
}

/// How much the bands count in the onset envelope: the kick most.
const WEIGHTS: [f32; 3] = [1.5, 1.0, 1.0];

/// The onset envelope of mono `pcm`: its values, how many there are per
/// second, and the time of the first one (seconds from the start of `pcm`).
pub fn onset_envelope(pcm: &[f32], rate: u32) -> (Vec<f32>, f64, f64) {
    let (bands, envelope_rate, hop) = band_flux(pcm, rate);
    if bands.is_empty() {
        return (Vec::new(), envelope_rate, 0.0);
    }
    let flux: Vec<f32> = bands
        .iter()
        .map(|f| (0..3).map(|b| WEIGHTS[b] * f[b]).sum())
        .collect();
    let envelope = stand_out(&flux, envelope_rate);
    // A step's rise happened during it; the smoothing shows it a little late.
    (envelope, envelope_rate, envelope_first(hop, rate))
}

/// The time of an envelope's first value, in seconds from the start of its
/// audio.
fn envelope_first(hop: usize, rate: u32) -> f64 {
    0.5 * hop as f64 / rate as f64 - 0.004
}

/// The value at a place between two samples (0 outside).
fn at(values: &[f32], index: f64) -> f64 {
    if index < 0.0 {
        return 0.0;
    }
    let i = index as usize;
    if i + 1 >= values.len() {
        return values.get(i).copied().unwrap_or(0.0) as f64;
    }
    let t = index - i as f64;
    values[i] as f64 * (1.0 - t) + values[i + 1] as f64 * t
}

/// The best phase of a pulse train of `period` steps over `envelope`: the
/// offset (in steps), the mean it collects there, and the mean over all
/// offsets.
fn best_phase(envelope: &[f32], period: f64) -> (f64, f64, f64) {
    let offsets = period.ceil() as usize;
    let (mut best, mut best_score, mut sum) = (0usize, f64::MIN, 0.0);
    let mut scores = Vec::with_capacity(offsets);
    for offset in 0..offsets {
        let (mut score, mut beats, mut index) = (0.0, 0u32, offset as f64);
        while index < (envelope.len() - 1) as f64 {
            score += at(envelope, index);
            beats += 1;
            index += period;
        }
        let score = score / beats.max(1) as f64;
        scores.push(score);
        sum += score;
        if score > best_score {
            (best, best_score) = (offset, score);
        }
    }
    // Between the steps: the top of the parabola through the best and its
    // neighbours.
    let n = scores.len();
    let (before, after) = (scores[(best + n - 1) % n], scores[(best + 1) % n]);
    let curve = before - 2.0 * best_score + after;
    let shift = if curve < 0.0 {
        (0.5 * (before - after) / curve).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    (best as f64 + shift, best_score, sum / n as f64)
}

/// The beat grid of mono `pcm` at `rate`, which starts `start` seconds into
/// its track. `near`, a tempo already known for the track (from another part
/// of it), keeps the answer within a few percent of that. `None`: too short,
/// or silent.
pub fn analyse(pcm: &[f32], rate: u32, start: f64, near: Option<f64>) -> Option<BeatGrid> {
    if rate == 0 || (pcm.len() as f64) < MIN_AUDIO * rate as f64 {
        return None;
    }
    let (raw, envelope_rate, first) = onset_envelope(pcm, rate);
    if raw.len() < 16 {
        return None;
    }
    // A touch of smoothing: an onset counts for a beat 5 ms beside it too.
    let envelope: Vec<f32> = (0..raw.len())
        .map(|i| {
            let before = raw[i.saturating_sub(1)];
            let after = raw[(i + 1).min(raw.len() - 1)];
            0.25 * before + 0.5 * raw[i] + 0.25 * after
        })
        .collect();
    let n = envelope.len();

    let (slowest, fastest) = match near {
        Some(bpm) => (bpm * (1.0 - PRIOR_RANGE), bpm * (1.0 + PRIOR_RANGE)),
        None => (MIN_BPM, MAX_BPM),
    };
    let lag_of = |bpm: f64| envelope_rate * 60.0 / bpm;
    let (shortest, longest) = (lag_of(fastest), lag_of(slowest));
    // The autocorrelation, as far as four beats of the slowest tempo (or as
    // far as half the audio goes).
    const TEETH: usize = 4;
    let lags = ((longest * TEETH as f64).ceil() as usize + 2).min(n / 2);
    if (lags as f64) < longest + 2.0 {
        return None;
    }
    let mean = envelope.iter().sum::<f32>() / n as f32;
    let centred: Vec<f32> = envelope.iter().map(|v| v - mean).collect();
    let correlation: Vec<f32> = (0..lags)
        .map(|lag| {
            let sum: f32 = centred[..n - lag]
                .iter()
                .zip(&centred[lag..])
                .map(|(a, b)| a * b)
                .sum();
            sum / (n - lag) as f32
        })
        .collect();
    if correlation[0] <= 1e-12 {
        return None;
    }

    // The comb: a beat period shows at its multiples too.
    let comb = |lag: f64| {
        let (mut score, mut teeth) = (0.0, 0);
        for tooth in 1..=TEETH {
            let place = lag * tooth as f64;
            if place < (lags - 1) as f64 {
                score += at(&correlation, place);
                teeth += 1;
            }
        }
        score / teeth.max(1) as f64 / correlation[0] as f64
    };
    let related = |lag: f64| at(&correlation, lag) / correlation[0] as f64;
    let mut best = (shortest, f64::MIN);
    let mut lag = shortest;
    while lag <= longest {
        let mut score = comb(lag);
        if near.is_none() {
            // Beats divide in two far more often than in three: with strong
            // off-beats, one and a half beats would otherwise do as well.
            score += 0.5 * (related(lag / 2.0) - related(lag / 3.0).max(related(lag * 2.0 / 3.0)));
            let octaves = (envelope_rate * 60.0 / lag / USUAL_BPM).log2();
            score *= (-0.5 * (octaves / USUAL_SPREAD_OCTAVES).powi(2)).exp();
        }
        if score > best.1 {
            best = (lag, score);
        }
        lag += 0.25;
    }

    // Refine against the envelope itself: over the whole window the beats
    // must stay in step, which pins the period down to a small part of a step.
    let coarse = best.0;
    let mut refined = (coarse, f64::MIN, 0.0, 0.0);
    for step in -40..=40 {
        let period = coarse * (1.0 + step as f64 * 0.0005);
        let (offset, score, mean) = best_phase(&envelope, period);
        if score > refined.1 {
            refined = (period, score, offset, mean);
        }
    }
    let (period, score, offset, _) = refined;
    if score <= 0.0 {
        return None;
    }
    let bpm = envelope_rate * 60.0 / period;
    let period_seconds = 60.0 / bpm;
    let anchor = start + first + offset / envelope_rate;
    Some(BeatGrid {
        bpm,
        // (The earliest beat at or after the window's start.)
        anchor: anchor - ((anchor - start) / period_seconds).floor() * period_seconds,
        // How alike the envelope is to itself a beat (two, three, four) on.
        confidence: comb(period).clamp(0.0, 1.0),
    })
}

/// "Start on the Beat": how far into a track its entry is looked for, in
/// seconds. Intros of dance tracks are 16 or 32 bars (30 or 60 s at 128
/// BPM, and the long ones are on extended mixes); what only gets going
/// after a minute has usually had a verse by then, which is not an intro
/// to skip.
pub const ENTRY_SEARCH: f64 = 60.0;
/// ...and never further than this share of the track.
pub const ENTRY_SHARE: f64 = 0.3;
/// How much of a track's start the search wants to see, in seconds: as far
/// as it looks, and what has to follow an entry.
pub const ENTRY_WINDOW: f64 = ENTRY_SEARCH + ENTRY_AFTER;
/// What follows an entry is looked at this far: its tempo and beats.
const ENTRY_AFTER: f64 = 16.0;
/// An entry this early is none: the track is played from its start.
const ENTRY_EARLIEST: f64 = 5.0;
/// The drive is read over this many seconds at a time...
const DRIVE_WINDOW: f64 = 4.0;
/// ...and has to stay up for this long after an entry.
const DRIVE_HOLD: f64 = 8.0;
/// "Going" is this share of the drive of the track's strongest tenth.
const DRIVE_GOING: f64 = 0.7;
/// The beat after an entry has to be this clear (see `BeatGrid::confidence`).
const ENTRY_CONFIDENCE: f64 = 0.2;
/// Without a steady beat to count, an entry needs a quiet start: the drive
/// of the seconds before it at most this share of "going".
const QUIET: f64 = 0.45;
const QUIET_BEFORE: f64 = 4.0;
/// Start this long before the entry's beat, so its attack is whole.
const ENTRY_LEAD: f64 = 0.03;

/// Where a crossfade brings a track in ("Start on the Beat").
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Seconds into the track; 0: from its start.
    pub start: f64,
    /// The beats from there on (more exact there than the intro's grid, and
    /// there at all when the intro has none).
    pub grid: Option<BeatGrid>,
    pub found: EntryFound,
}

/// What the search for an entry came to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EntryFound {
    /// The drums come in here, this many beats after the track's first
    /// sound (when that could be counted).
    Later { beats_in: Option<u32> },
    /// The track gets going within its first seconds.
    FromTheStart,
    /// It gets going at this time, which is too far in to skip to.
    TooFar(f64),
    /// Where it gets going there is no clear beat.
    NoClearBeat,
    /// Too little audio, or silence.
    Nothing,
}

impl std::fmt::Display for EntryFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Later { beats_in: Some(n) } => write!(f, "the beat comes in {n} beats in"),
            Self::Later { beats_in: None } => write!(f, "the beat comes in"),
            Self::FromTheStart => write!(f, "it gets going from its start"),
            Self::TooFar(at) => write!(f, "it only gets going at {at:.0} s, too far in"),
            Self::NoClearBeat => write!(f, "no clear beat that comes in and stays"),
            Self::Nothing => write!(f, "nothing to go by"),
        }
    }
}

impl Entry {
    fn at_start(found: EntryFound, grid: Option<BeatGrid>) -> Self {
        Self {
            start: 0.0,
            grid,
            found,
        }
    }
}

/// Where the track of mono `pcm` at `rate` (from its very beginning, about
/// [`ENTRY_WINDOW`] seconds of it) gets going: where its drums come in and
/// stay. `length` is the whole track's, in seconds, when known.
///
/// The *drive* of a moment is how many and how strong the onsets of the
/// four seconds after it are, the kick's band counting most. A track is
/// going when its drive is at 70 % of what its strongest tenth has (of the
/// part seen), and stays about there for eight seconds. From that moment,
/// which is only good to a second or two, the entry is the beat at which
/// the onsets on the beats step up the most. That is where a part begins,
/// so it is a phrase's first beat without bars being counted; when the
/// track's first sound is a whole number of beats before it, that number
/// is given (32, 64: a check, for the log).
pub fn find_entry(pcm: &[f32], rate: u32, length: Option<f64>) -> Entry {
    let nothing = Entry::at_start(EntryFound::Nothing, None);
    if rate == 0 || (pcm.len() as f64) < (DRIVE_HOLD + DRIVE_WINDOW + MIN_AUDIO) * rate as f64 {
        return nothing;
    }
    let (bands, envelope_rate, hop) = band_flux(pcm, rate);
    if bands.is_empty() {
        return nothing;
    }
    let first = envelope_first(hop, rate);
    // Drums are kick and hats; voices, pads and guitars are in the middle.
    let flux: Vec<f32> = bands
        .iter()
        .map(|f| 1.5 * f[0] + 0.5 * f[1] + f[2])
        .collect();
    let drive_envelope = stand_out(&flux, envelope_rate);
    let n = drive_envelope.len();
    let mut sums = Vec::with_capacity(n + 1);
    sums.push(0.0f64);
    for &v in &drive_envelope {
        sums.push(sums.last().unwrap() + v as f64);
    }
    let step = (0.5 * envelope_rate) as usize;
    let window = (DRIVE_WINDOW * envelope_rate) as usize;
    let drive: Vec<f64> = (0..)
        .map(|k| k * step)
        .take_while(|from| from + window <= n)
        .map(|from| (sums[from + window] - sums[from]) / window as f64)
        .collect();
    let mut sorted = drive.clone();
    sorted.sort_by(f64::total_cmp);
    let strong = sorted[sorted.len() * 9 / 10];
    let going = DRIVE_GOING * strong;
    if going <= 0.0 {
        return nothing;
    }
    // The first moment from which it is going, and stays so.
    let hold = (DRIVE_HOLD / 0.5) as usize;
    let Some(rough) = (0..drive.len().saturating_sub(hold)).find(|&k| {
        drive[k] >= going
            && drive[k..=k + hold].iter().all(|d| *d >= 0.75 * going)
            && drive[k..=k + hold].iter().sum::<f64>() >= going * (hold + 1) as f64
    }) else {
        return Entry::at_start(EntryFound::NoClearBeat, None);
    };
    let rough = rough as f64 * 0.5;
    let cut = |from: f64, seconds: f64| {
        let a = ((from * rate as f64) as usize).min(pcm.len());
        let b = (((from + seconds) * rate as f64) as usize).min(pcm.len());
        &pcm[a..b]
    };
    if rough < ENTRY_EARLIEST {
        return Entry::at_start(EntryFound::FromTheStart, None);
    }
    let furthest = length.map_or(ENTRY_SEARCH, |l| (l * ENTRY_SHARE).min(ENTRY_SEARCH));
    if rough > furthest + DRIVE_WINDOW {
        return Entry::at_start(EntryFound::TooFar(rough), None);
    }
    // The beats of what follows (the drive rises up to four seconds before
    // the drums are there, so this looks from a little later).
    let from = rough + 0.5 * DRIVE_WINDOW;
    let Some(grid) = analyse(cut(from, ENTRY_AFTER), rate, from, None)
        .filter(|grid| grid.is_usable() && grid.confidence >= ENTRY_CONFIDENCE)
    else {
        // No steady beat to count (a swung or a played one). After a start
        // that was plainly quiet, the first big onset will do all the same.
        let at = |seconds: f64| ((seconds * envelope_rate) as usize).min(n);
        let before = (sums[at(rough)] - sums[at(rough - QUIET_BEFORE)]) / at(QUIET_BEFORE) as f64;
        let (a, b) = (at(rough), at(rough + DRIVE_WINDOW + 1.0));
        let most = drive_envelope[a..b].iter().copied().fold(0.0f32, f32::max);
        let onset = drive_envelope[a..b].iter().position(|v| *v >= 0.5 * most);
        return match onset {
            Some(index) if before <= QUIET * going && most > 0.0 => {
                let time = first + (a + index) as f64 / envelope_rate;
                if time > furthest {
                    Entry::at_start(EntryFound::TooFar(time), None)
                } else {
                    Entry {
                        start: (time - ENTRY_LEAD).max(0.0),
                        grid: None,
                        found: EntryFound::Later { beats_in: None },
                    }
                }
            }
            _ => Entry::at_start(EntryFound::NoClearBeat, None),
        };
    };
    // How strong the onset on a beat is: the most within 40 ms of it.
    let period = grid.period();
    let reach = (0.04 * envelope_rate) as usize;
    let on_beat = |beat: i64| -> Option<f64> {
        let time = grid.anchor + beat as f64 * period;
        let index = ((time - first) * envelope_rate).round();
        if index < 0.0 || index as usize >= n {
            return None;
        }
        let index = index as usize;
        let (a, b) = (index.saturating_sub(reach), (index + reach + 1).min(n));
        Some(drive_envelope[a..b].iter().copied().fold(0.0f32, f32::max) as f64)
    };
    // The beat where four beats and the four before them differ the most,
    // from four bars before the rough moment to two bars after.
    let around = ((rough + DRIVE_WINDOW - grid.anchor) / period).round() as i64;
    let mean = |beats: std::ops::Range<i64>| -> Option<f64> {
        let values: Vec<f64> = beats.filter_map(on_beat).collect();
        (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
    };
    let mut best: Option<(i64, f64)> = None;
    for beat in around - 16..=around + 8 {
        if grid.anchor + beat as f64 * period < 0.0 {
            continue;
        }
        let (Some(after), Some(here)) = (mean(beat..beat + 4), on_beat(beat)) else {
            continue;
        };
        let before = mean(beat - 4..beat).unwrap_or(0.0);
        // (The beat itself has to be one of the strong ones.)
        let step = after - before;
        if here >= 0.5 * after && best.is_none_or(|(_, most)| step > most) {
            best = Some((beat, step));
        }
    }
    let Some((beat, _)) = best else {
        return Entry::at_start(EntryFound::NoClearBeat, Some(grid));
    };
    // Counted from the track's first sound, when that is on these beats
    // (the first onset a tenth as strong as the entry's beats).
    let level = 0.1 * mean(beat..beat + 4).unwrap_or(0.0);
    let beats_in = drive_envelope
        .iter()
        .position(|v| *v as f64 >= level)
        .map(|index| first + index as f64 / envelope_rate)
        .map(|sound| (grid.anchor + beat as f64 * period - sound) / period)
        .filter(|beats| *beats >= 1.0 && (beats - beats.round()).abs() < 0.15)
        .map(|beats| beats.round() as u32);
    let time = grid.anchor + beat as f64 * period;
    if time < ENTRY_EARLIEST {
        return Entry::at_start(EntryFound::FromTheStart, Some(grid));
    }
    if time > furthest {
        return Entry::at_start(EntryFound::TooFar(time), None);
    }
    Entry {
        start: (time - ENTRY_LEAD).max(0.0),
        // (Its anchor: the entry's own beat.)
        grid: Some(BeatGrid {
            anchor: time,
            ..grid
        }),
        found: EntryFound::Later { beats_in },
    }
}

/// How an incoming track is played to match the one that ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeatMatch {
    /// The incoming track's playback rate (1.03: three percent faster).
    pub rate: f64,
    /// The incoming track counts at this many times its own tempo: 1, or 2
    /// or 0.5 when its beats are matched to every other one.
    pub multiple: f64,
    /// Seconds of the incoming track from one matched beat to the next.
    pub period_in: f64,
    /// Seconds of the outgoing track from one beat to the next.
    pub period_out: f64,
    /// The rate the outgoing track plays at (1, unless it is still stretched
    /// from its own fade in).
    pub rate_out: f64,
}

/// Why two tracks are not beat-matched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoMatch {
    /// A tempo is outside [`MIN_BPM`]..[`MAX_BPM`].
    OutOfRange(f64),
    /// Even at half or double tempo, the stretch would be this much (0.12:
    /// twelve percent).
    TooFar(f64),
}

impl std::fmt::Display for NoMatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfRange(bpm) => write!(f, "a tempo of {bpm:.1} BPM is out of range"),
            Self::TooFar(by) => write!(
                f,
                "the tempos are {:.1} % apart (at most {:.0} % is stretched)",
                by * 100.0,
                MAX_STRETCH * 100.0
            ),
        }
    }
}

/// The rate at which a track of `incoming_bpm` matches one of
/// `outgoing_bpm` that plays at `outgoing_rate`, also trying it at half and
/// double its tempo, if that is a stretch of at most `max_stretch`.
pub fn match_tempo(
    outgoing_bpm: f64,
    outgoing_rate: f64,
    incoming_bpm: f64,
    max_stretch: f64,
) -> Result<BeatMatch, NoMatch> {
    let heard_bpm = outgoing_bpm * outgoing_rate;
    if !(0.5..=2.0).contains(&outgoing_rate) {
        return Err(NoMatch::OutOfRange(outgoing_bpm * outgoing_rate));
    }
    for bpm in [outgoing_bpm, incoming_bpm] {
        // (Twice as lenient as the analysis: a stretched track's tempo counts.)
        if !(MIN_BPM / 2.0..=MAX_BPM * 2.0).contains(&bpm) || !bpm.is_finite() {
            return Err(NoMatch::OutOfRange(bpm));
        }
    }
    let (multiple, rate) = [1.0, 2.0, 0.5]
        .into_iter()
        .map(|multiple| (multiple, heard_bpm / (incoming_bpm * multiple)))
        .min_by(|a, b| a.1.ln().abs().total_cmp(&b.1.ln().abs()))
        .expect("three candidates");
    if (rate - 1.0).abs() > max_stretch {
        return Err(NoMatch::TooFar(rate - 1.0));
    }
    Ok(BeatMatch {
        rate,
        multiple,
        period_in: 60.0 / (incoming_bpm * multiple),
        period_out: 60.0 / outgoing_bpm,
        rate_out: outgoing_rate,
    })
}

impl BeatMatch {
    /// How far the incoming track's beats are from the outgoing track's, in
    /// seconds as heard (positive: the incoming beats come early), with the
    /// outgoing track at `position_out` and the incoming one at `position_in`.
    pub fn error(
        &self,
        outgoing: &BeatGrid,
        position_out: f64,
        incoming: &BeatGrid,
        position_in: f64,
    ) -> f64 {
        let phase_out = ((position_out - outgoing.anchor) / self.period_out).rem_euclid(1.0);
        let phase_in = ((position_in - incoming.anchor) / self.period_in).rem_euclid(1.0);
        let ahead = (phase_in - phase_out + 0.5).rem_euclid(1.0) - 0.5;
        ahead * self.beat()
    }

    /// Seconds from one beat to the next, as heard during the fade.
    pub fn beat(&self) -> f64 {
        self.period_out / self.rate_out
    }

    /// How long to wait before starting the incoming track so that the beats
    /// land together: at least `at_least` seconds, less than a beat more.
    ///
    /// The outgoing track is at `position_out`. The incoming one waits at `position_in`; it plays at its own tempo up to
    /// `matched_from` (what its player has already taken in) and at
    /// [`Self::rate`] from there.
    pub fn start_delay(
        &self,
        outgoing: &BeatGrid,
        position_out: f64,
        incoming: &BeatGrid,
        position_in: f64,
        matched_from: f64,
        at_least: f64,
    ) -> f64 {
        let matched_from = matched_from.max(position_in);
        // Started now, the incoming track is at `matched_from` after `lead`
        // seconds, and from then on both keep their distance.
        let lead = matched_from - position_in;
        let early = self.error(
            outgoing,
            position_out + lead * self.rate_out,
            incoming,
            matched_from,
        );
        // Early beats are put right by starting that much later.
        let beat = self.beat();
        let wait = early.rem_euclid(beat);
        if wait < at_least {
            wait + beat * ((at_least - wait) / beat).ceil()
        } else {
            wait
        }
    }
}

/// What is kept on disk about a track's beats.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackBeats {
    version: u32,
    /// The grid of the track's first seconds; `None`: analysed, no beat found.
    pub intro: Option<BeatGrid>,
    /// How many seconds were analysed.
    pub analysed: f64,
    /// Where a crossfade brings the track in; `None`: not looked for (it is
    /// only with "Start on the Beat" on).
    #[serde(default)]
    pub entry: Option<Entry>,
}

impl TrackBeats {
    pub fn new(intro: Option<BeatGrid>, analysed: f64) -> Self {
        Self {
            version: CACHE_VERSION,
            intro,
            analysed,
            entry: None,
        }
    }

    pub fn with_entry(mut self, entry: Entry) -> Self {
        self.entry = Some(entry);
        self
    }

    /// Where a crossfade starts the track: its entry, when one was found.
    pub fn entry_start(&self) -> f64 {
        self.entry.map_or(0.0, |entry| entry.start)
    }

    /// The beats a crossfade that starts the track at `start` goes by: the
    /// entry's, when it starts there, else the intro's.
    pub fn grid_at(&self, start: f64) -> Option<BeatGrid> {
        match self.entry {
            // (Also when it has none: the intro's beats say nothing there.)
            Some(Entry {
                start: entry, grid, ..
            }) if entry > 0.0 && (start - entry).abs() < 1.0 => grid,
            _ => self.intro,
        }
    }
}

fn cache_file(dir: &Path, video_id: &str) -> Option<std::path::PathBuf> {
    let safe = !video_id.is_empty()
        && video_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    safe.then(|| dir.join(format!("{video_id}.json")))
}

/// The kept analysis of a video, if there is one of this version.
pub fn load(dir: &Path, video_id: &str) -> Option<TrackBeats> {
    let text = std::fs::read_to_string(cache_file(dir, video_id)?).ok()?;
    serde_json::from_str::<TrackBeats>(&text)
        .ok()
        .filter(|beats| beats.version == CACHE_VERSION)
}

/// Keep a video's analysis.
pub fn store(dir: &Path, video_id: &str, beats: &TrackBeats) -> std::io::Result<()> {
    let Some(path) = cache_file(dir, video_id) else {
        return Err(std::io::Error::other("not a video id"));
    };
    std::fs::create_dir_all(dir)?;
    let part = path.with_extension("json.part");
    std::fs::write(
        &part,
        serde_json::to_vec(beats).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(part, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = ANALYSIS_RATE;

    /// A small generator of noise that is the same on every run.
    struct Noise(u32);

    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    /// `seconds` of a drum loop at `bpm` whose first kick is at `first`:
    /// kicks on the beats, quieter hats between them (`hats` of the kick's
    /// level), and a noise floor.
    fn drums(bpm: f64, first: f64, seconds: f64, hats: f32) -> Vec<f32> {
        let mut noise = Noise(0x2545_f491);
        let n = (seconds * RATE as f64) as usize;
        let mut pcm: Vec<f32> = (0..n).map(|_| 0.01 * noise.next()).collect();
        let period = 60.0 / bpm;
        let mut hit = |at: f64, level: f32, kick: bool| {
            let from = (at * RATE as f64).round() as usize;
            let length = (if kick { 0.12 } else { 0.03 } * RATE as f64) as usize;
            for i in 0..length {
                let Some(sample) = pcm.get_mut(from + i) else {
                    break;
                };
                let t = i as f32 / RATE as f32;
                *sample += level
                    * if kick {
                        (std::f32::consts::TAU * 70.0 * t).sin() * (-t / 0.04).exp()
                    } else {
                        noise.next() * (-t / 0.008).exp()
                    };
            }
        };
        let mut beat = 0;
        while first + beat as f64 * period < seconds {
            hit(first + beat as f64 * period, 0.8, true);
            if hats > 0.0 {
                hit(first + (beat as f64 + 0.5) * period, 0.8 * hats, false);
            }
            beat += 1;
        }
        pcm
    }

    /// How far `grid`'s beats are from beats at `first + k * period`, in
    /// seconds (the nearest beat counts).
    fn off_beat(grid: &BeatGrid, first: f64, period: f64) -> f64 {
        let phase = ((grid.anchor - first) / period).rem_euclid(1.0);
        phase.min(1.0 - phase) * period
    }

    #[test]
    fn finds_the_tempo_and_the_beats_of_a_drum_loop() {
        for (bpm, first) in [
            (100.0, 0.21),
            (120.0, 0.0),
            (128.0, 0.37),
            (140.0, 0.1),
            (93.5, 0.3),
        ] {
            let grid = analyse(&drums(bpm, first, 30.0, 0.3), RATE, 0.0, None).unwrap();
            assert!(
                (grid.bpm / bpm - 1.0).abs() < 0.004,
                "{bpm} BPM came out as {:.2}",
                grid.bpm
            );
            let off = off_beat(&grid, first, 60.0 / bpm);
            assert!(off < 0.012, "{bpm} BPM: beats {:.1} ms off", off * 1000.0);
            assert!(
                grid.confidence > 0.6,
                "{bpm} BPM: confidence {}",
                grid.confidence
            );
            assert!(grid.anchor >= 0.0 && grid.anchor < grid.period() + 1e-9);
        }
    }

    #[test]
    fn a_window_later_in_the_track_keeps_the_tracks_clock() {
        // The same loop, cut from 61.3 s on: the grid is in track time.
        let (bpm, first): (f64, f64) = (124.0, 0.15);
        let period = 60.0 / bpm;
        let start: f64 = 61.3;
        let shifted = first - start;
        let pcm = drums(bpm, shifted.rem_euclid(period), 20.0, 0.3);
        let grid = analyse(&pcm, RATE, start, None).unwrap();
        assert!((grid.bpm / bpm - 1.0).abs() < 0.004);
        assert!(off_beat(&grid, first, period) < 0.012);
        assert!(grid.anchor >= start && grid.anchor < start + period + 1e-9);
    }

    #[test]
    fn half_and_double_tempo_are_told_apart_or_matched_anyway() {
        // Slow and fast loops may come out at twice or half their tempo...
        for bpm in [64.0, 70.0, 85.0, 170.0, 180.0] {
            let grid = analyse(&drums(bpm, 0.2, 40.0, 0.5), RATE, 0.0, None).unwrap();
            let ratio = grid.bpm / bpm;
            let multiple = [0.5, 1.0, 2.0]
                .into_iter()
                .find(|m| (ratio / m - 1.0).abs() < 0.005);
            assert!(multiple.is_some(), "{bpm} BPM came out as {:.2}", grid.bpm);
            assert!((MIN_BPM..=MAX_BPM).contains(&grid.bpm));
            // ...and either way they match a track at the true tempo.
            let matched = match_tempo(bpm, 1.0, grid.bpm, MAX_STRETCH).unwrap();
            assert!((matched.rate - 1.0).abs() < 0.005);
            // The beats found are beats (or half beats) of the loop.
            assert!(off_beat(&grid, 0.2, 30.0 / bpm) < 0.012);
        }
        // Usual tempos come out as they are, even with loud off-beats.
        for bpm in [96.0, 110.0, 126.0, 150.0] {
            let grid = analyse(&drums(bpm, 0.2, 40.0, 0.9), RATE, 0.0, None).unwrap();
            assert!(
                (grid.bpm / bpm - 1.0).abs() < 0.005,
                "{bpm} BPM came out as {:.2}",
                grid.bpm
            );
        }
    }

    #[test]
    fn a_known_tempo_keeps_a_short_window_on_it() {
        // Twelve seconds, as the DJ has of the track that ends: with the
        // tempo of its start to go by, the answer stays in that octave.
        let pcm = drums(70.0, 0.2, 12.0, 0.6);
        let grid = analyse(&pcm, RATE, 0.0, Some(70.5)).unwrap();
        assert!((grid.bpm / 70.0 - 1.0).abs() < 0.01, "{}", grid.bpm);
        let grid = analyse(&pcm, RATE, 0.0, Some(140.0)).unwrap();
        assert!((grid.bpm / 140.0 - 1.0).abs() < 0.01, "{}", grid.bpm);
    }

    #[test]
    fn noise_and_silence_have_no_beat() {
        assert!(analyse(&vec![0.0; 20 * RATE as usize], RATE, 0.0, None).is_none());
        assert!(analyse(&[0.1; 100], RATE, 0.0, None).is_none());
        let mut noise = Noise(7);
        let hiss: Vec<f32> = (0..20 * RATE as usize)
            .map(|_| 0.3 * noise.next())
            .collect();
        assert!(analyse(&hiss, RATE, 0.0, None).is_none_or(|grid| !grid.is_usable()));
    }

    #[test]
    fn tempos_match_within_the_stretch_also_at_half_and_double() {
        let m = match_tempo(128.0, 1.0, 124.0, MAX_STRETCH).unwrap();
        assert!((m.rate - 128.0 / 124.0).abs() < 1e-12);
        assert_eq!(m.multiple, 1.0);
        assert!((m.period_in / m.rate - m.period_out).abs() < 1e-12);

        // 87 against 170: every other beat of the fast one.
        let m = match_tempo(87.0, 1.0, 170.0, MAX_STRETCH).unwrap();
        assert_eq!(m.multiple, 0.5);
        assert!((m.rate - 87.0 / 85.0).abs() < 1e-12);
        // 140 against 72: its half beats count too.
        let m = match_tempo(140.0, 1.0, 72.0, MAX_STRETCH).unwrap();
        assert_eq!(m.multiple, 2.0);
        assert!((m.period_in / m.rate - m.period_out).abs() < 1e-12);

        assert!(matches!(
            match_tempo(128.0, 1.0, 100.0, MAX_STRETCH),
            Err(NoMatch::TooFar(by)) if (by - 0.28).abs() < 1e-9
        ));
        assert!(matches!(
            match_tempo(128.0, 1.0, 0.0, MAX_STRETCH),
            Err(NoMatch::OutOfRange(_))
        ));
        assert!(match_tempo(120.0, 1.0, 111.0, MAX_STRETCH).is_err());
        assert!(match_tempo(120.0, 1.0, 112.0, MAX_STRETCH).is_ok());
        // An outgoing track that is itself stretched is matched as heard.
        let m = match_tempo(120.0, 1.05, 126.0, MAX_STRETCH).unwrap();
        assert!((m.rate - 1.0).abs() < 1e-12);
        assert!((m.beat() - 60.0 / 126.0).abs() < 1e-12);
    }

    #[test]
    fn the_start_is_delayed_until_the_beats_land_together() {
        let outgoing = BeatGrid {
            bpm: 126.0,
            anchor: 200.13,
            confidence: 0.8,
        };
        let incoming = BeatGrid {
            bpm: 120.0,
            anchor: 0.31,
            confidence: 0.8,
        };
        for (position_out, rate_out, position_in, matched_from) in [
            (231.07, 1.0, 0.0, 1.15),
            (231.5, 1.0, 0.08, 1.2),
            (240.0, 1.02, 0.0, 0.0),
        ] {
            let m = match_tempo(outgoing.bpm, rate_out, incoming.bpm, MAX_STRETCH).unwrap();
            let wait = m.start_delay(
                &outgoing,
                position_out,
                &incoming,
                position_in,
                matched_from,
                0.03,
            );
            assert!(wait >= 0.03 && wait < 0.03 + outgoing.period() / rate_out + 1e-9);
            // Play it through: some seconds in, the beats are together.
            let heard = 5.0;
            let since_start = heard - wait;
            let lead = matched_from - position_in;
            let at_in = matched_from + (since_start - lead) * m.rate;
            let at_out = position_out + heard * rate_out;
            let error = m.error(&outgoing, at_out, &incoming, at_in);
            assert!(error.abs() < 1e-6, "{error}");
        }
    }

    #[test]
    fn the_error_says_which_track_is_early() {
        let grid = |anchor| BeatGrid {
            bpm: 120.0,
            anchor,
            confidence: 1.0,
        };
        let m = match_tempo(120.0, 1.0, 120.0, MAX_STRETCH).unwrap();
        // The incoming track 20 ms further than a beat: its beats came early.
        let error = m.error(&grid(0.0), 10.0, &grid(0.0), 3.02);
        assert!((error - 0.02).abs() < 1e-9);
        let error = m.error(&grid(0.0), 10.0, &grid(0.0), 2.98);
        assert!((error + 0.02).abs() < 1e-9);
    }

    #[test]
    fn stereo_is_mixed_down_and_thinned_out() {
        let stereo: Vec<f32> = (0..44100 * 2)
            .map(|i| if i % 2 == 0 { 1.0 } else { 0.0 })
            .collect();
        let (mono, rate) = downmix(&stereo, 2, 44100, ANALYSIS_RATE);
        assert_eq!(rate, 11025);
        assert_eq!(mono.len(), 11025);
        assert!(mono.iter().all(|v| (v - 0.5).abs() < 1e-6));
    }

    /// A track at `bpm` whose beats start at 0.2 s: a pad all along, quiet
    /// hats on the beats from beat `hats` on (if any), and the drums (kicks
    /// on the beats, hats between them) from beat `kicks` on.
    fn track(bpm: f64, seconds: f64, hats: Option<u32>, kicks: Option<u32>) -> Vec<f32> {
        let period = 60.0 / bpm;
        let mut pcm: Vec<f32> = (0..(seconds * RATE as f64) as usize)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let swell = 0.6 + 0.4 * (std::f32::consts::TAU * 0.11 * t).sin();
                0.06 * swell
                    * t.min(1.0)
                    * [220.0, 277.2, 329.6]
                        .iter()
                        .map(|hz| (std::f32::consts::TAU * hz * t).sin())
                        .sum::<f32>()
            })
            .collect();
        let mut noise = Noise(0x9e37_79b9);
        let mut tick = |at: f64, level: f32| {
            let from = (at * RATE as f64).round() as usize;
            for i in 0..(0.03 * RATE as f64) as usize {
                if let Some(sample) = pcm.get_mut(from + i) {
                    *sample += level * noise.next() * (-(i as f32 / RATE as f32) / 0.008).exp();
                }
            }
        };
        if let Some(hats) = hats {
            let mut beat = hats;
            while 0.2 + beat as f64 * period < seconds {
                tick(0.2 + beat as f64 * period, 0.3);
                beat += 1;
            }
        }
        if let Some(kicks) = kicks {
            let from = 0.2 + kicks as f64 * period;
            let start = ((from * RATE as f64).round() as usize).min(pcm.len());
            for (sample, drum) in
                pcm[start..]
                    .iter_mut()
                    .zip(drums(bpm, 0.0, (seconds - from).max(0.0), 0.4))
            {
                *sample += drum;
            }
        }
        pcm
    }

    #[test]
    fn the_entry_is_where_the_drums_come_in() {
        // A pad for sixteen bars, then the drums: bar 17 it is.
        for (bpm, kicks) in [(128.0, 64), (120.0, 32), (100.0, 48), (140.0, 96)] {
            let period = 60.0 / bpm;
            let at = 0.2 + kicks as f64 * period;
            let pcm = track(bpm, ENTRY_WINDOW, None, Some(kicks));
            let entry = find_entry(&pcm, RATE, Some(300.0));
            assert!(
                (entry.start + ENTRY_LEAD - at).abs() < 0.015,
                "{bpm} BPM: the drums come in at {at:.3} s, the entry is at {:.3} s ({})",
                entry.start,
                entry.found
            );
            assert!(matches!(entry.found, EntryFound::Later { .. }));
            // Its grid is the drums': in tempo and on their beats.
            let grid = entry.grid.unwrap();
            assert!((grid.bpm / bpm - 1.0).abs() < 0.004, "{}", grid.bpm);
            assert!(off_beat(&grid, at, period) < 0.012);
            assert!(grid.is_usable());
        }
    }

    #[test]
    fn the_entry_is_counted_from_the_first_sound() {
        // Quiet hats from the first beat, the drums 32 beats later.
        let pcm = track(126.0, ENTRY_WINDOW, Some(0), Some(32));
        let entry = find_entry(&pcm, RATE, None);
        let at = 0.2 + 32.0 * 60.0 / 126.0;
        assert!((entry.start + ENTRY_LEAD - at).abs() < 0.015, "{entry:?}");
        assert_eq!(entry.found, EntryFound::Later { beats_in: Some(32) });
    }

    #[test]
    fn a_track_that_starts_with_its_drums_starts_at_its_start() {
        for kicks in [0, 4, 8] {
            let entry = find_entry(&track(124.0, ENTRY_WINDOW, None, Some(kicks)), RATE, None);
            assert_eq!(entry.start, 0.0, "{entry:?}");
            assert_eq!(entry.found, EntryFound::FromTheStart);
        }
        // Drums alone, as the other tests have them.
        let entry = find_entry(&drums(128.0, 0.37, 60.0, 0.3), RATE, None);
        assert_eq!((entry.start, entry.found), (0.0, EntryFound::FromTheStart));
    }

    #[test]
    fn nothing_is_skipped_without_drums_or_too_far_in() {
        // A pad and nothing else, silence, a few seconds: from the start.
        let entry = find_entry(&track(120.0, ENTRY_WINDOW, None, None), RATE, None);
        assert_eq!(entry.start, 0.0, "{entry:?}");
        let silence = vec![0.0; 60 * RATE as usize];
        assert_eq!(find_entry(&silence, RATE, None).found, EntryFound::Nothing);
        assert_eq!(
            find_entry(&[0.1; 100], RATE, None).found,
            EntryFound::Nothing
        );
        // The drums of a two-minute track at 52.7 s: more than its 30 %.
        let pcm = track(128.0, ENTRY_WINDOW, None, Some(112));
        let entry = find_entry(&pcm, RATE, Some(120.0));
        assert_eq!(entry.start, 0.0);
        assert!(
            matches!(entry.found, EntryFound::TooFar(at) if (at - 52.7).abs() < 7.0),
            "{entry:?}"
        );
        // The same in a six-minute track is an entry.
        assert!((find_entry(&pcm, RATE, Some(360.0)).start - 52.67).abs() < 0.015);
        // Drums that only come after the search's reach are not looked at.
        let late = track(128.0, 120.0, None, Some(180));
        assert_eq!(find_entry(&late, RATE, Some(600.0)).start, 0.0);
    }

    #[test]
    fn a_fade_goes_by_the_beats_of_where_it_starts() {
        let grid = |anchor| BeatGrid {
            bpm: 120.0,
            anchor,
            confidence: 0.8,
        };
        let entry = Entry {
            start: 30.17,
            grid: Some(grid(30.2)),
            found: EntryFound::Later { beats_in: Some(64) },
        };
        let beats = TrackBeats::new(Some(grid(0.3)), ENTRY_WINDOW).with_entry(entry);
        assert_eq!(beats.entry_start(), 30.17);
        assert_eq!(beats.grid_at(30.17), Some(grid(30.2)));
        assert_eq!(beats.grid_at(0.0), Some(grid(0.3)));
        // Not looked for: the start, and the intro's beats.
        let plain = TrackBeats::new(Some(grid(0.3)), INTRO);
        assert_eq!(plain.entry_start(), 0.0);
        assert_eq!(plain.grid_at(0.0), Some(grid(0.3)));
    }

    #[test]
    fn analyses_are_kept_by_video_id() {
        let dir = std::env::temp_dir().join(format!("kinora-beats-test-{}", std::process::id()));
        let beats = TrackBeats::new(
            Some(BeatGrid {
                bpm: 123.4,
                anchor: 0.25,
                confidence: 0.7,
            }),
            INTRO,
        );
        assert_eq!(load(&dir, "abc_DEF-123"), None);
        store(&dir, "abc_DEF-123", &beats).unwrap();
        assert_eq!(load(&dir, "abc_DEF-123"), Some(beats));
        // "No beat" is an answer too.
        store(&dir, "none", &TrackBeats::new(None, INTRO)).unwrap();
        assert_eq!(load(&dir, "none").unwrap().intro, None);
        // An entry is kept with it.
        let entry = Entry {
            start: 30.17,
            grid: beats.intro,
            found: EntryFound::Later { beats_in: Some(64) },
        };
        store(&dir, "entry", &beats.with_entry(entry)).unwrap();
        assert_eq!(load(&dir, "entry").unwrap().entry, Some(entry));
        // What an older version kept is not read.
        std::fs::write(
            dir.join("old.json"),
            r#"{"version":1,"intro":null,"analysed":45.0}"#,
        )
        .unwrap();
        assert_eq!(load(&dir, "old"), None);
        assert!(store(&dir, "../escape", &beats).is_err());
        assert_eq!(load(&dir, "../escape"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
