// SPDX-License-Identifier: GPL-3.0-or-later

//! Kinora: the audio side of the DJ's deck (see `docs/architecture.md`,
//! "The deck's audio"): what scratching and beat matching need from a
//! player, which Clapper does not offer.
//!
//! - [`Tap`]: a small bin set as the player's audio filter. It keeps the
//!   last seconds of what the player decoded (for the scratch, and for the
//!   beats of what plays), and it changes the tempo without a seek: Clapper's
//!   own `speed` is a flushing seek, a tenth of a second of silence at best,
//!   and it changes the pitch. Here the segment that reaches `scaletempo` is
//!   rewritten on the fly, so the tempo changes between two buffers, at the
//!   same pitch, and the player's position stays true.
//! - [`decode`]: a stretch of a track as PCM, decoded by a pipeline of its
//!   own (on the caller's thread: not the main one).
//!
//! Only the `gstreamer` crate is used: the data is read with pad probes, so
//! no further GStreamer libraries are linked.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gst::prelude::*;

/// How much of what played is kept, in seconds.
const KEPT: f64 = 12.0;
/// Tempo changes smaller than this are not worth a new segment.
const RATE_STEP: f64 = 0.0005;
/// The track's own tempo for a tap that is [engaged](Tap::set_engaged): as
/// good as 1, but `scaletempo` stays at work. Going between exactly 1 (where
/// it only passes audio through) and any other rate costs about 50 ms of
/// audio each time, which the sound device then puts right a second later
/// with a gap (measured: see docs/architecture.md); between two other rates
/// nothing is lost.
const OWN_TEMPO: f64 = 1.00002;
/// Stretched audio is heard a little off where its segment puts it: by this
/// many seconds per unit of rate away from 1 (at 0.94, 30 ms late; at 1.06,
/// 30 ms early). Measured with a click track, see docs/architecture.md.
pub const STRETCH_LAG: f64 = 0.5;

/// Decoded audio: interleaved 32-bit float samples.
#[derive(Debug, Clone, Default)]
pub struct Pcm {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub channels: usize,
    /// Where the first sample is in its track, in seconds.
    pub start: f64,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.rate.max(1) as f64
    }

    /// Where the audio ends in its track, in seconds.
    pub fn end(&self) -> f64 {
        self.start + self.seconds()
    }
}

fn samples_of(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// What the tap keeps of the audio that went through it: stereo frames in a
/// ring, without a gap between them.
struct Ring {
    samples: Vec<f32>,
    /// Where the next frame goes, and how many are there, in frames.
    head: usize,
    filled: usize,
    rate: u32,
    /// The track time after the last frame.
    end: f64,
}

impl Ring {
    fn capacity(&self) -> usize {
        self.samples.len() / 2
    }

    fn write(&mut self, bytes: &[u8], channels: usize, rate: u32, at: f64) {
        if rate == 0 || channels == 0 {
            return;
        }
        let wanted = (KEPT * rate as f64) as usize * 2;
        if self.rate != rate || self.samples.len() != wanted {
            self.samples = vec![0.0; wanted];
            (self.rate, self.head, self.filled) = (rate, 0, 0);
        }
        // A seek, or a gap: what was kept does not lead up to this.
        if (at - self.end).abs() > 0.003 {
            self.filled = 0;
        }
        let capacity = self.capacity();
        let mut frames = 0;
        let mut samples = samples_of(bytes);
        while let Some(left) = samples.next() {
            let right = if channels > 1 {
                samples.next().unwrap_or(left)
            } else {
                left
            };
            for _ in 2..channels {
                samples.next();
            }
            self.samples[self.head * 2] = left;
            self.samples[self.head * 2 + 1] = right;
            self.head = (self.head + 1) % capacity;
            frames += 1;
        }
        self.filled = (self.filled + frames).min(capacity);
        self.end = at + frames as f64 / rate as f64;
    }

    fn snapshot(&self) -> Option<Pcm> {
        if self.filled == 0 {
            return None;
        }
        let capacity = self.capacity();
        let from = (self.head + capacity - self.filled) % capacity;
        let mut samples = Vec::with_capacity(self.filled * 2);
        if from + self.filled <= capacity {
            samples.extend_from_slice(&self.samples[from * 2..(from + self.filled) * 2]);
        } else {
            samples.extend_from_slice(&self.samples[from * 2..]);
            samples.extend_from_slice(&self.samples[..(from + self.filled - capacity) * 2]);
        }
        Some(Pcm {
            samples,
            rate: self.rate,
            channels: 2,
            start: self.end - self.filled as f64 / self.rate as f64,
        })
    }
}

/// The tempo, as the tap last told `scaletempo`.
struct Tempo {
    /// The wanted rate, times the player's own (1 unless its speed is set).
    want: f64,
    /// The rate in force, and the segment that says so.
    sent: f64,
    segment: Option<gst::FormattedSegment<gst::ClockTime>>,
    /// The rate of the player's own segment.
    upstream: f64,
    /// The track time after the last buffer that went through.
    flow: Option<f64>,
    /// The segments since the last seek, oldest first: from which time on
    /// the pipeline's clock (nanoseconds of running time), from where in the
    /// track (nanoseconds), at which rate.
    since_seek: Vec<(u64, u64, f64)>,
}

impl Tempo {
    fn remember(&mut self, segment: &gst::FormattedSegment<gst::ClockTime>, fresh: bool) {
        if fresh {
            self.since_seek.clear();
        } else if self.since_seek.len() >= 64 {
            self.since_seek.remove(0);
        }
        if let (Some(base), Some(time)) = (segment.base(), segment.time()) {
            self.since_seek
                .push((base.nseconds(), time.nseconds(), segment.rate()));
        }
    }
}

struct Shared {
    recording: AtomicBool,
    engaged: AtomicBool,
    rate: AtomicU32,
    channels: AtomicU32,
    ring: Mutex<Ring>,
    tempo: Mutex<Tempo>,
}

impl Shared {
    /// The rate `scaletempo` is given for a wanted one.
    fn rate_for(&self, want: f64) -> f64 {
        if self.engaged.load(Ordering::Relaxed) && (want - 1.0).abs() < OWN_TEMPO - 1.0 {
            OWN_TEMPO
        } else {
            want
        }
    }
}

/// See the module's documentation.
pub struct Tap {
    bin: gst::Element,
    shared: Arc<Shared>,
    stretches: bool,
}

impl Tap {
    /// `None` when GStreamer lacks the plain elements this is made of.
    pub fn new() -> Option<Self> {
        let stretches = gst::ElementFactory::find("scaletempo").is_some();
        let description = format!(
            "audioconvert ! capsfilter caps=audio/x-raw,format=F32LE,layout=interleaved \
             ! identity name=tap silent=true{}",
            if stretches { " ! scaletempo" } else { "" }
        );
        let bin = gst::parse::bin_from_description(&description, true)
            .inspect_err(|e| log::warn!("No deck audio: {e}"))
            .ok()?;
        let identity = bin.by_name("tap")?;
        let (sink, src) = (identity.static_pad("sink")?, identity.static_pad("src")?);
        let shared = Arc::new(Shared {
            recording: AtomicBool::new(false),
            engaged: AtomicBool::new(false),
            rate: AtomicU32::new(0),
            channels: AtomicU32::new(0),
            ring: Mutex::new(Ring {
                samples: Vec::new(),
                head: 0,
                filled: 0,
                rate: 0,
                end: f64::MIN,
            }),
            tempo: Mutex::new(Tempo {
                want: 1.0,
                sent: 1.0,
                segment: None,
                upstream: 1.0,
                flow: None,
                since_seek: Vec::new(),
            }),
        });

        // The player's segments and format, as they come by.
        sink.add_probe(
            gst::PadProbeType::EVENT_DOWNSTREAM,
            glib::clone!(
                #[strong]
                shared,
                move |_, info| {
                    let replacement = match info.event().map(|e| e.view()) {
                        Some(gst::EventView::Caps(caps)) => {
                            if let Some(s) = caps.caps().structure(0) {
                                let number = |field| s.get::<i32>(field).unwrap_or(0).max(0) as u32;
                                shared.rate.store(number("rate"), Ordering::Relaxed);
                                shared.channels.store(number("channels"), Ordering::Relaxed);
                            }
                            None
                        }
                        Some(gst::EventView::Segment(event)) => event
                            .segment()
                            .downcast_ref::<gst::ClockTime>()
                            .and_then(|segment| {
                                let mut tempo = shared.tempo.lock().unwrap();
                                let mut segment = segment.clone();
                                let rate = shared.rate_for(tempo.want);
                                tempo.upstream = segment.rate();
                                tempo.sent = rate;
                                tempo.flow = None;
                                let changed = rate != 1.0;
                                if changed {
                                    segment.set_rate(tempo.upstream * rate);
                                }
                                tempo.segment = Some(segment.clone());
                                tempo.remember(&segment, true);
                                changed.then(|| {
                                    gst::event::Segment::builder(&segment)
                                        .seqnum(event.seqnum())
                                        .build()
                                })
                            }),
                        _ => None,
                    };
                    if let Some(event) = replacement {
                        info.data = Some(gst::PadProbeData::Event(event));
                    }
                    gst::PadProbeReturn::Ok
                }
            ),
        );

        // Every buffer: a new tempo goes out ahead of it, and it is kept.
        sink.add_probe(
            gst::PadProbeType::BUFFER,
            glib::clone!(
                #[strong]
                shared,
                move |_, info| {
                    let Some(buffer) = info.buffer() else {
                        return gst::PadProbeReturn::Ok;
                    };
                    let (Some(pts), rate, channels) = (
                        buffer.pts(),
                        shared.rate.load(Ordering::Relaxed),
                        shared.channels.load(Ordering::Relaxed) as usize,
                    ) else {
                        return gst::PadProbeReturn::Ok;
                    };
                    if rate == 0 || channels == 0 {
                        return gst::PadProbeReturn::Ok;
                    }
                    let mut tempo = shared.tempo.lock().unwrap();
                    let Some(segment) = tempo.segment.clone() else {
                        return gst::PadProbeReturn::Ok;
                    };
                    let (Some(start), Some(time), Some(base)) =
                        (segment.start(), segment.time(), segment.base())
                    else {
                        return gst::PadProbeReturn::Ok;
                    };
                    let since = pts.saturating_sub(start);
                    let at = (time + since).nseconds() as f64 / 1e9;
                    let frames = buffer.size() / (4 * channels);
                    tempo.flow = Some(at + frames as f64 / rate as f64);
                    let tempo_now = shared.rate_for(tempo.want);
                    let event = (tempo_now != tempo.sent
                        && ((tempo_now - tempo.sent).abs() >= RATE_STEP
                            || tempo_now == shared.rate_for(1.0)))
                    .then(|| {
                        // From this buffer on: the same place in the
                        // track and on the clock, at another rate.
                        let mut next = segment.clone();
                        let run = (since.nseconds() as f64 / segment.rate().abs()) as u64;
                        next.set_base(base + gst::ClockTime::from_nseconds(run));
                        next.set_time(time + since);
                        next.set_start(pts);
                        next.set_position(pts);
                        next.set_rate(tempo.upstream * tempo_now);
                        tempo.sent = tempo_now;
                        tempo.segment = Some(next.clone());
                        tempo.remember(&next, false);
                        gst::event::Segment::new(&next)
                    });
                    drop(tempo);
                    if let Some(event) = event {
                        src.push_event(event);
                    }
                    if shared.recording.load(Ordering::Relaxed)
                        && let Ok(map) = buffer.map_readable()
                    {
                        shared
                            .ring
                            .lock()
                            .unwrap()
                            .write(map.as_slice(), channels, rate, at);
                    }
                    gst::PadProbeReturn::Ok
                }
            ),
        );
        Some(Self {
            bin: bin.upcast(),
            shared,
            stretches,
        })
    }

    /// The element to set as the player's audio filter.
    pub fn element(&self) -> &gst::Element {
        &self.bin
    }

    /// Keep what plays (the DJ's pages do).
    pub fn set_recording(&self, recording: bool) {
        self.shared.recording.store(recording, Ordering::Relaxed);
    }

    /// The last seconds that went through, as far back as they are gapless.
    /// They reach a little past what is heard: the sound device has not
    /// played all of it yet.
    pub fn snapshot(&self) -> Option<Pcm> {
        self.shared.ring.lock().unwrap().snapshot()
    }

    /// Be ready to change the tempo (see [`OWN_TEMPO`]): to be said before
    /// the player starts, and left alone afterwards. A tap that is not
    /// engaged passes the player's audio through untouched.
    pub fn set_engaged(&self, engaged: bool) {
        self.shared
            .engaged
            .store(engaged && self.stretches, Ordering::Relaxed);
    }

    /// The tempo can be changed without a break.
    pub fn stretches(&self) -> bool {
        self.shared.engaged.load(Ordering::Relaxed)
    }

    /// Play at `rate` times the track's tempo, at the same pitch. It takes
    /// effect from the audio that goes through next: [`Self::flow`] says from
    /// where in the track, which is heard about a second later.
    pub fn set_rate(&self, rate: f64) {
        if self.stretches() {
            self.shared.tempo.lock().unwrap().want = rate.clamp(0.5, 2.0);
        }
    }

    pub fn rate(&self) -> f64 {
        self.shared.tempo.lock().unwrap().want
    }

    /// The track time up to which audio went through (at the tempo of then).
    pub fn flow(&self) -> Option<f64> {
        self.shared.tempo.lock().unwrap().flow
    }

    /// The position that is heard right now, in seconds, to a few
    /// milliseconds (Clapper's own `position` is refreshed a few times a
    /// second).
    ///
    /// While it plays, this is the pipeline's clock read through the
    /// segments this tap sent: the position the sound device reports is off
    /// by up to 50 ms for a second and more around every change of tempo (it
    /// goes by the newest segment, whose audio is not heard yet).
    pub fn position(&self) -> Option<f64> {
        let mut top: gst::Object = self.bin.clone().upcast();
        while let Some(parent) = top.parent() {
            top = parent;
        }
        let pipeline = top.downcast::<gst::Element>().ok()?;
        let playing = matches!(
            pipeline.state(gst::ClockTime::ZERO),
            (Ok(gst::StateChangeSuccess::Success), gst::State::Playing, _)
        );
        let by_clock = playing
            .then(|| pipeline.current_running_time())
            .flatten()
            .and_then(|now| {
                let now = now.nseconds();
                let tempo = self.shared.tempo.lock().unwrap();
                let &(base, time, rate) = tempo
                    .since_seek
                    .iter()
                    .rev()
                    .find(|(base, ..)| *base <= now)
                    .or(tempo.since_seek.first())?;
                let run = now.saturating_sub(base) as f64 / 1e9;
                Some(time as f64 / 1e9 + run * rate.abs() + STRETCH_LAG * (rate.abs() - 1.0))
            });
        by_clock.or_else(|| {
            pipeline
                .query_position::<gst::ClockTime>()
                .map(|position| position.nseconds() as f64 / 1e9)
        })
    }
}

/// What [`decode`] hands its audio to, piece by piece; `false` stops it.
pub type Pieces = Arc<dyn Fn(Pcm) -> bool + Send + Sync>;

/// Decode `seconds` of `uri` from `start` on, as `channels` channels at
/// `rate`. This blocks until it is done (a second or so for a prepared
/// stream): call it from a thread of its own. What was decoded before an
/// error or `timeout` is returned all the same, if there is any.
///
/// With `pieces`, the audio is handed over as it comes, a quarter of a
/// second at a time, and what is returned is empty.
pub fn decode(
    uri: &str,
    start: f64,
    seconds: f64,
    rate: u32,
    channels: usize,
    timeout: Duration,
    pieces: Option<Pieces>,
) -> Result<Pcm, String> {
    let make = |factory: &str| {
        gst::ElementFactory::make(factory)
            .build()
            .map_err(|_| format!("no {factory} element"))
    };
    let pipeline = gst::Pipeline::new();
    let source = make("uridecodebin3")?;
    source.set_property("uri", uri);
    let (convert, resample, filter, sink) = (
        make("audioconvert")?,
        make("audioresample")?,
        make("capsfilter")?,
        make("fakesink")?,
    );
    filter.set_property(
        "caps",
        gst::Caps::builder("audio/x-raw")
            .field("format", "F32LE")
            .field("layout", "interleaved")
            .field("rate", rate as i32)
            .field("channels", channels as i32)
            .build(),
    );
    sink.set_property("sync", false);
    pipeline
        .add_many([&source, &convert, &resample, &filter, &sink])
        .and_then(|_| gst::Element::link_many([&convert, &resample, &filter, &sink]))
        .map_err(|e| e.to_string())?;
    // The first audio stream; a video's picture is left alone.
    let audio_in = convert.static_pad("sink").ok_or("no pad")?;
    source.connect_pad_added(move |_, pad| {
        if pad.name().starts_with("audio") && !audio_in.is_linked() {
            let _ = pad.link(&audio_in);
        }
    });

    struct Taken {
        pcm: Pcm,
        begun: bool,
        /// How many samples were handed over already.
        handed: usize,
    }
    let taken = Arc::new(Mutex::new(Taken {
        pcm: Pcm {
            rate,
            channels,
            start,
            ..Default::default()
        },
        begun: false,
        handed: 0,
    }));
    let done = Arc::new(AtomicBool::new(false));
    // Take what is there out of `taken`, leaving it ready for what follows.
    fn hand_over(taken: &mut Taken) -> Pcm {
        let next = Pcm {
            samples: Vec::new(),
            start: taken.pcm.end(),
            ..taken.pcm
        };
        taken.handed += taken.pcm.samples.len();
        std::mem::replace(&mut taken.pcm, next)
    }
    let end = start + seconds;
    sink.static_pad("sink").ok_or("no pad")?.add_probe(
        gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_FLUSH,
        glib::clone!(
            #[strong]
            taken,
            #[strong]
            done,
            #[strong]
            pieces,
            move |pad, info| {
                let mut taken = taken.lock().unwrap();
                let Some(buffer) = info.buffer() else {
                    // A seek: what came before it is from somewhere else.
                    taken.pcm.samples.clear();
                    taken.begun = false;
                    return gst::PadProbeReturn::Ok;
                };
                let at = pad
                    .sticky_event::<gst::event::Segment>(0)
                    .and_then(|event| {
                        let segment = event.segment().downcast_ref::<gst::ClockTime>()?;
                        segment.to_stream_time(buffer.pts()?)
                    })
                    .map(|time| time.nseconds() as f64 / 1e9);
                let Some(at) = at else {
                    return gst::PadProbeReturn::Ok;
                };
                if at >= end {
                    done.store(true, Ordering::Relaxed);
                    return gst::PadProbeReturn::Ok;
                }
                if !taken.begun {
                    taken.begun = true;
                    taken.pcm.start = at;
                }
                if let Ok(map) = buffer.map_readable() {
                    taken.pcm.samples.extend(samples_of(map.as_slice()));
                }
                if let Some(pieces) = &pieces
                    && taken.pcm.seconds() >= 0.25
                    && !pieces(hand_over(&mut taken))
                {
                    done.store(true, Ordering::Relaxed);
                }
                gst::PadProbeReturn::Ok
            }
        ),
    );

    let bus = pipeline.bus().ok_or("no bus")?;
    let deadline = Instant::now() + timeout;
    // Wait for the pipeline to settle (`true`), an error, or the end.
    let wait = |until_done: bool| -> Result<bool, String> {
        loop {
            if until_done && done.load(Ordering::Relaxed) {
                return Ok(true);
            }
            if Instant::now() > deadline {
                return Err("timed out".into());
            }
            let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(40)) else {
                continue;
            };
            match message.view() {
                gst::MessageView::Error(e) => return Err(e.error().to_string()),
                gst::MessageView::Eos(_) => return Ok(false),
                gst::MessageView::AsyncDone(_) if !until_done => return Ok(true),
                _ => {}
            }
        }
    };
    let run = || -> Result<(), String> {
        pipeline
            .set_state(gst::State::Paused)
            .map_err(|e| e.to_string())?;
        if !wait(false)? {
            return Ok(());
        }
        if start > 0.01 {
            pipeline
                .seek_simple(
                    gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                    gst::ClockTime::from_nseconds((start * 1e9) as u64),
                )
                .map_err(|e| e.to_string())?;
            if !wait(false)? {
                return Ok(());
            }
        }
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| e.to_string())?;
        wait(true).map(|_| ())
    };
    let result = run();
    let _ = pipeline.set_state(gst::State::Null);
    let mut taken = taken.lock().unwrap();
    if let Some(pieces) = &pieces
        && !taken.pcm.samples.is_empty()
    {
        pieces(hand_over(&mut taken));
    }
    let nothing = taken.handed + taken.pcm.samples.len() == 0;
    match result {
        Err(e) if nothing => Err(e),
        _ if nothing => Err("nothing decoded".into()),
        _ => Ok(std::mem::take(&mut taken.pcm)),
    }
}

/// Kinora: what the Dev panel reads (see `gui/devtools`); development builds only.
#[cfg(feature = "dev-hooks")]
impl Tap {
    /// The sample rate and the channels of what goes through.
    pub fn dev_format(&self) -> (u32, u32) {
        (
            self.shared.rate.load(Ordering::Relaxed),
            self.shared.channels.load(Ordering::Relaxed),
        )
    }

    /// The tap stays at work for tempo changes (see [`Tap::set_engaged`]).
    pub fn dev_engaged(&self) -> bool {
        self.shared.engaged.load(Ordering::Relaxed)
    }

    /// The player's pipeline, once the tap is in it.
    pub fn dev_pipeline(&self) -> Option<gst::Element> {
        let mut top: gst::Object = self.bin.clone().upcast();
        while let Some(parent) = top.parent() {
            top = parent;
        }
        top.downcast::<gst::Element>()
            .ok()
            .filter(|top| *top != self.bin)
    }

    /// What was kept after the track time `from`: stereo frames, oldest
    /// first, in one or two pieces, each with the rate and where it starts.
    /// Nothing is copied.
    pub fn dev_read(&self, from: f64, take: &mut dyn FnMut(&[f32], u32, f64)) {
        let ring = self.shared.ring.lock().unwrap();
        if ring.filled == 0 || ring.rate == 0 {
            return;
        }
        let rate = ring.rate as f64;
        let start = ring.end - ring.filled as f64 / rate;
        let skip = (((from - start).max(0.0) * rate) as usize).min(ring.filled);
        let frames = ring.filled - skip;
        if frames == 0 {
            return;
        }
        let capacity = ring.capacity();
        let first = (ring.head + capacity - frames) % capacity;
        let at = ring.end - frames as f64 / rate;
        if first + frames <= capacity {
            take(&ring.samples[first * 2..(first + frames) * 2], ring.rate, at);
        } else {
            let head = capacity - first;
            take(&ring.samples[first * 2..], ring.rate, at);
            take(
                &ring.samples[..(frames - head) * 2],
                ring.rate,
                at + head as f64 / rate,
            );
        }
    }
}
