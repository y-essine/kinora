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
const CACHE_VERSION: u32 = 1;

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

/// The onset envelope of mono `pcm`: its values, how many there are per
/// second, and the time of the first one (seconds from the start of `pcm`).
pub fn onset_envelope(pcm: &[f32], rate: u32) -> (Vec<f32>, f64, f64) {
    let hop = ((rate as f64 / ENVELOPE_RATE).round() as usize).max(1);
    let envelope_rate = rate as f64 / hop as f64;
    let steps = pcm.len() / hop;
    if steps < 8 {
        return (Vec::new(), envelope_rate, 0.0);
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
        return (Vec::new(), envelope_rate, 0.0);
    }
    // Loudness, not energy: a kick under a loud pad still counts.
    let loud = |e: f32| (1.0 + 20.0 * e / total).ln();
    const WEIGHTS: [f32; 3] = [1.5, 1.0, 1.0];
    let mut flux = vec![0.0f32; steps];
    for i in 1..steps {
        flux[i] = (0..3)
            .map(|b| WEIGHTS[b] * (loud(bands[i][b]) - loud(bands[i - 1][b])).max(0.0))
            .sum();
    }
    // What stands out from the mean of the 0.4 s around it.
    let half = (0.2 * envelope_rate) as usize;
    let mut sums = Vec::with_capacity(steps + 1);
    sums.push(0.0f64);
    for &f in &flux {
        sums.push(sums.last().unwrap() + f as f64);
    }
    let envelope = (0..steps)
        .map(|i| {
            let (from, to) = (i.saturating_sub(half), (i + half + 1).min(steps));
            let mean = (sums[to] - sums[from]) / (to - from) as f64;
            (flux[i] as f64 - mean).max(0.0) as f32
        })
        .collect();
    // A step's rise happened during it; the smoothing shows it a little late.
    let first = 0.5 * hop as f64 / rate as f64 - 0.004;
    (envelope, envelope_rate, first)
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
}

impl TrackBeats {
    pub fn new(intro: Option<BeatGrid>, analysed: f64) -> Self {
        Self {
            version: CACHE_VERSION,
            intro,
            analysed,
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
        assert!(store(&dir, "../escape", &beats).is_err());
        assert_eq!(load(&dir, "../escape"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
