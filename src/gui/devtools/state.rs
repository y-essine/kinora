// SPDX-License-Identifier: GPL-3.0-or-later

//! The Dev panel's model: one [`Snapshot`] of what the app is doing, taken
//! on the main thread. The panel draws it, `dev-dump` writes it as JSON:
//! what is on screen and what is in the file are the same numbers.
//!
//! The JSON is described in CONTRIBUTING-KINORA.md ("Dev panel"); change
//! both together, and raise [`SCHEMA`] when a field changes its meaning.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use adw::prelude::*;
use glib::subclass::types::ObjectSubclassIsExt;
use kinora_core::beats::{BeatGrid, TrackBeats};
use kinora_core::dj::{DjSettings, Track};
use serde::Serialize;

use super::{DjProbe, FadeSample, logs, peaks};
use crate::backend::{Client, Video};
use crate::gui::video_page::VideoPage;
use crate::gui::window::Window;

pub const SCHEMA: u32 = 1;

/// How many log lines a dump carries.
const DUMP_LOG: usize = 300;

#[derive(Serialize, Default, Clone)]
pub struct Snapshot {
    pub schema: u32,
    /// Local time, RFC 3339.
    pub written: String,
    /// Seconds since the app started; the log's `at` and the main loop's
    /// samples count from the same moment.
    pub uptime: f64,
    pub app: App,
    pub process: Process,
    pub main_loop: MainLoop,
    pub dj: Dj,
    /// Deck A and deck B. A track stays on its deck while it plays.
    pub decks: Vec<Deck>,
    /// The crossfade that runs, or the last one.
    pub fade: Option<Fade>,
    /// The queue's first tracks.
    pub queue: Vec<QueueItem>,
    /// The video in the player.
    pub video: Option<VideoMeta>,
    pub notes: logs::Notes,
    /// What the open panel itself costs (zeros while it is closed).
    pub panel: PanelCost,
    /// The log's tail (dumps only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<LogLine>,
}

/// Milliseconds of the main thread: taking a snapshot and setting the
/// labels (5 times a second), drawing the waveforms (20 times a second).
#[derive(Serialize, Default, Clone, Copy)]
pub struct PanelCost {
    pub collect_ms: f64,
    pub show_ms: f64,
    pub draw_ms: f64,
}

#[derive(Serialize, Default, Clone)]
pub struct App {
    pub version: String,
    pub app_id: String,
    pub profile: String,
    pub pid: u32,
    pub gtk: String,
    pub gstreamer: String,
    pub panel_open: bool,
    /// The `KH_*` hooks and `RUST_LOG` the app was started with.
    pub env: BTreeMap<String, String>,
}

#[derive(Serialize, Default, Clone)]
pub struct Process {
    pub rss_mb: f64,
    pub peak_rss_mb: f64,
    /// Of one core, over the last second or so.
    pub cpu_percent: f64,
    pub threads: u32,
    /// Threads by name (numbers dropped), most first.
    pub thread_names: Vec<(String, u32)>,
    pub open_files: u32,
    /// Video pages alive, each with its own player.
    pub players: Vec<PlayerInfo>,
}

#[derive(Serialize, Default, Clone)]
pub struct PlayerInfo {
    pub id: String,
    pub state: String,
    /// "current", "outgoing", "incoming", "warm", "parked" (a DJ page
    /// waiting to be used again), or "retired" (let go,
    /// kept a few seconds).
    pub role: String,
    pub age: f64,
}

#[derive(Serialize, Default, Clone)]
pub struct MainLoop {
    /// How late timers came over the last minute, in milliseconds.
    pub lag_ms_mean: f64,
    pub lag_ms_max: f64,
    /// Timers 100 ms late or more, since the app started.
    pub stalls: u32,
    /// (uptime, milliseconds late), dumps only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<(f64, f32)>,
}

#[derive(Serialize, Default, Clone)]
pub struct Dj {
    pub on: bool,
    pub settings: Option<DjSettings>,
    pub seed: Option<String>,
    pub source: Option<String>,
    pub queued: usize,
    pub played: usize,
    pub starting: bool,
    pub refilling: bool,
    pub hovered: Option<String>,
    pub warm: Vec<WarmInfo>,
}

#[derive(Serialize, Default, Clone)]
pub struct WarmInfo {
    pub id: String,
    pub ready: bool,
    /// Seconds since it was warmed.
    pub since: f64,
}

#[derive(Serialize, Default, Clone)]
pub struct Deck {
    /// "A" or "B".
    pub deck: String,
    /// "live", "out", "in" (a crossfade), "next" (loaded ahead), "queued"
    /// (the next track, without a player yet) or "video" (no DJ).
    pub role: String,
    pub id: String,
    pub title: String,
    pub artist: String,
    /// One word: "playing", "paused", "buffering", "stopped", "warming",
    /// "warm", "held" (warm, waits for its beat), "scratching", or for a
    /// queued track "cold", "preparing", "prepared".
    pub status: String,
    /// The player's own state.
    pub player: String,
    pub loaded: bool,
    /// Seconds: where the track is heard, and how long it is.
    pub position: f64,
    pub duration: Option<f64>,
    /// Seconds downloaded past the position.
    pub buffered_ahead: Option<f64>,
    /// The track time up to which audio was decoded (a tempo set now is
    /// heard from there).
    pub decoded_to: Option<f64>,
    /// Clapper's volume (cubic), the user's while the DJ fades, and the
    /// fade's linear gain (1 outside a fade).
    pub volume: f64,
    pub base_volume: Option<f64>,
    pub gain: f64,
    /// The rate the track plays at (1.016: 1.6 % faster).
    pub tempo: f64,
    pub can_stretch: bool,
    pub stream: Stream,
    pub beats: BeatsInfo,
    /// The beats drawn on the deck, and where they are from.
    pub grid: Option<Grid>,
    /// "Start on the Beat": where the player was sent, and whether it is there.
    pub entry_target: Option<f64>,
    pub entry_ready: bool,
    /// Warmed ahead: for how long, and ready or not.
    pub warm: Option<WarmInfo>,
    pub waveform: Waveform,
}

#[derive(Serialize, Default, Clone)]
pub struct Stream {
    /// "prepared HLS audio", "prepared HLS", "fast start", "yt-dlp
    /// fallback", "downloaded file", or "none".
    pub kind: String,
    pub uri: Option<String>,
    /// A prepared manifest on disk: is it there, and how large.
    pub file_bytes: Option<u64>,
    pub audio_only: bool,
    pub codec: Option<String>,
    pub sample_rate: u32,
    pub channels: u32,
    pub bitrate: u32,
}

#[derive(Serialize, Default, Clone)]
pub struct BeatsInfo {
    /// "cached", "no beat" (analysed, none found), "analysing", "failed",
    /// "pending" (wanted, not asked for yet) or "off".
    pub status: String,
    pub bpm: Option<f64>,
    pub first_beat: Option<f64>,
    pub confidence: Option<f64>,
    /// Clear enough to match by.
    pub usable: bool,
    pub analysed_seconds: Option<f64>,
    pub entry: Option<EntryInfo>,
    pub error: Option<String>,
}

#[derive(Serialize, Default, Clone)]
pub struct EntryInfo {
    pub start: f64,
    pub found: String,
    pub bpm: Option<f64>,
    pub confidence: Option<f64>,
}

#[derive(Serialize, Default, Clone)]
pub struct Grid {
    pub bpm: f64,
    pub anchor: f64,
    pub confidence: f64,
    /// "intro", "entry" (the analysis kept on disk) or "tail" / "fade" (read
    /// during a crossfade).
    pub from: String,
}

#[derive(Serialize, Default, Clone)]
pub struct Waveform {
    pub known_seconds: f64,
    /// "whole track", "decoding", "live only", "decode failed: …".
    pub source: String,
}

#[derive(Serialize, Default, Clone)]
pub struct Fade {
    /// It runs now; else this is the last one.
    pub live: bool,
    /// "starting" (the incoming track is not playing yet), "fading", "done".
    pub phase: String,
    pub outgoing: Option<String>,
    pub incoming: Option<String>,
    /// The length asked for, and seconds since it was asked.
    pub wanted: Option<f64>,
    pub waited: Option<f64>,
    pub elapsed: f64,
    pub total: f64,
    pub gain_out: f64,
    pub gain_in: f64,
    /// How ready the incoming track was: "warm", "warming", "prepared", "cold".
    pub readiness: Option<String>,
    pub to_entry: Option<String>,
    pub sync: Option<Sync>,
    /// The last reading of the beats' distance, in milliseconds.
    pub beat_ms: Option<f64>,
    pub beat_settled: bool,
    /// Every step of the fade (dumps only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<FadeSample>,
}

#[derive(Serialize, Default, Clone)]
pub struct Sync {
    /// "waiting", "resting", "held", "matched".
    pub stage: String,
    pub bpm_in: f64,
    pub bpm_out: Option<f64>,
    pub stretch_percent: Option<f64>,
    /// 1, or 2 / 0.5 when the incoming track is counted double or half.
    pub multiple: Option<f64>,
    /// Seconds since the incoming track was let go on the beat.
    pub let_go: Option<f64>,
    /// The readings that counted, in milliseconds.
    pub errors_ms: Vec<f64>,
    pub nudging: bool,
}

#[derive(Serialize, Default, Clone)]
pub struct QueueItem {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: Option<f64>,
    pub kind: String,
    /// "warm", "warming", "prepared", "preparing" or "cold".
    pub stream: String,
    pub beats: BeatsInfo,
}

#[derive(Serialize, Default, Clone)]
pub struct VideoMeta {
    pub id: String,
    pub platform: String,
    pub title: String,
    pub uploader: String,
    pub uploader_id: String,
    pub duration: Option<f64>,
    pub views: Option<u64>,
    pub likes: Option<u64>,
    pub live: bool,
    pub uploaded: String,
    pub downloaded: bool,
    pub dj_page: bool,
    pub audio_only: bool,
    pub prepared: bool,
    pub prepared_audio: bool,
}

#[derive(Serialize, Default, Clone)]
pub struct LogLine {
    pub at: f64,
    pub level: String,
    pub target: String,
    pub text: String,
}

/// When a page's buffer was asked about, what was buffered, and its demuxer.
type Buffered = (Instant, Option<f64>, Option<glib::WeakRef<gst::Element>>);
/// A page on a deck: as what, and the beats a running fade read for it.
type Shown = (VideoPage, &'static str, Option<(BeatGrid, &'static str)>);

/// Takes snapshots, and remembers between them what is slow to look up.
#[derive(Default)]
pub struct Collector {
    /// The tracks on deck A and deck B.
    slots: [Option<String>; 2],
    beats: HashMap<String, (Instant, Option<TrackBeats>)>,
    /// Per page: when it was asked, what is buffered, and its demuxer.
    buffer: HashMap<usize, Buffered>,
    process: Option<(Instant, Process)>,
    cpu: Option<(Instant, u64)>,
    app: Option<App>,
    /// The pages on deck A and deck B, as of the last snapshot.
    pages: [Option<glib::WeakRef<VideoPage>>; 2],
    /// Per page: what it plays, looked up once a second.
    streams: HashMap<usize, (Instant, Stream)>,
    /// The beat matching of the fade that ran last, and how ready its
    /// incoming track was.
    last_sync: Option<Sync>,
    last_readiness: Option<String>,
}

const SLOW: Duration = Duration::from_millis(1000);
const BUFFER_EVERY: Duration = Duration::from_millis(300);

fn state_name(state: clapper::PlayerState) -> &'static str {
    match state {
        clapper::PlayerState::Playing => "playing",
        clapper::PlayerState::Paused => "paused",
        clapper::PlayerState::Buffering => "buffering",
        _ => "stopped",
    }
}

fn artist_of(track: &Track) -> String {
    track
        .artists
        .iter()
        .map(|artist| artist.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a snapshot is for: the panel's page that shows (the rest is left
/// empty, it costs to look up), or everything (a dump, with the log's tail
/// and every sample).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum What {
    /// The decks, and their waveforms kept up to date.
    Decks,
    Queue,
    System,
    Log,
    All,
}

fn video_id(id: &str) -> pcore::VideoId {
    pcore::VideoId {
        id: id.to_owned(),
        platform: "youtube".into(),
    }
}

/// A track's prepared stream, as the DJ would play it.
fn prepared_uri(client: &Client, id: &str) -> Option<String> {
    let (core, id) = (client.core(), video_id(id));
    core.prepared_audio_uri(&id)
        .or_else(|| core.prepared_uri(&id))
}

impl Collector {
    pub fn collect(&mut self, window: &Window, what: What) -> Snapshot {
        let full = what == What::All;
        let client = window.client();
        let settings = client.core().dj_settings();
        let probe = window.imp().dj().dev_probe();
        let current = window.imp().current_video_page();
        let notes = logs::notes();

        // Which pages are on the decks, and as what.
        let mut shown: Vec<Shown> = Vec::new();
        let mut queued_deck = None;
        match probe.as_ref() {
            Some(dj) => {
                if let Some(fade) = dj.fade.as_ref() {
                    let sync = fade.sync.as_ref();
                    shown.push((
                        fade.outgoing.clone(),
                        "out",
                        sync.and_then(|sync| sync.outgoing)
                            .map(|grid| (grid, "tail")),
                    ));
                    shown.push((
                        fade.incoming.clone(),
                        "in",
                        sync.map(|sync| (sync.incoming, "fade")),
                    ));
                } else {
                    if let Some(page) = dj.page.clone().or(current.clone()) {
                        shown.push((page, "live", None));
                    }
                    if let Some(next) = dj.upcoming.first() {
                        match dj.warm.iter().find(|warm| warm.id == next.id) {
                            Some(warm) => shown.push((warm.page.clone(), "next", None)),
                            None => queued_deck = Some(next.clone()),
                        }
                    }
                }
            }
            None => {
                if let Some(page) = current.clone() {
                    shown.push((page, "video", None));
                }
            }
        }
        let analysed: &[String] = probe.as_ref().map_or(&[], |dj| &dj.analysed);
        if !matches!(what, What::Decks | What::All) {
            shown.clear();
            queued_deck = None;
        }
        let mut decks: Vec<Deck> = shown
            .iter()
            .filter_map(|(page, role, grid)| {
                let warm = probe
                    .as_ref()
                    .and_then(|dj| dj.warm.iter().find(|warm| warm.page == *page))
                    .map(|warm| WarmInfo {
                        id: warm.id.clone(),
                        ready: warm.ready,
                        since: warm.since,
                    });
                self.deck(
                    page,
                    role,
                    *grid,
                    warm,
                    &client,
                    &settings,
                    analysed,
                    &notes,
                    what == What::Decks,
                )
            })
            .collect();
        if let (Some(track), Some(dj)) = (queued_deck, probe.as_ref()) {
            decks.push(self.queued_deck(&track, dj, &client, &settings, &notes));
        }
        self.seat(&mut decks);
        self.pages = [0, 1].map(|seat| {
            let id = self.slots[seat].as_deref()?;
            shown
                .iter()
                .find(|(page, ..)| page.video().is_some_and(|video| video.id().id == id))
                .map(|(page, ..)| page.downgrade())
        });

        let listed = matches!(what, What::Queue | What::All);
        let queue = probe.as_ref().map_or(Vec::new(), |dj| {
            dj.upcoming
                .iter()
                .filter(|_| listed)
                .map(|track| QueueItem {
                    id: track.id.clone(),
                    title: track.title.clone(),
                    artist: artist_of(track),
                    duration: track.duration.map(|d| d.as_secs_f64()),
                    kind: format!("{:?}", track.kind),
                    stream: Self::readiness(track, dj, &client).to_owned(),
                    beats: self.beats_info(&track.id, &client, &settings, analysed, &notes),
                })
                .collect()
        });

        let mut fade = Self::fade(probe.as_ref(), full);
        match fade.as_mut() {
            Some(fade) if fade.live => {
                self.last_sync = fade.sync.clone();
                self.last_readiness = fade.readiness.clone();
            }
            Some(fade) => {
                fade.sync = self.last_sync.clone();
                fade.readiness = self.last_readiness.clone();
            }
            None => {}
        }
        // The fade's own gains (a player's volume says the same, unless the
        // user's volume is 0).
        if let Some(fade) = fade.as_ref().filter(|fade| fade.live) {
            for deck in decks.iter_mut() {
                match deck.role.as_str() {
                    "out" => deck.gain = fade.gain_out,
                    "in" => deck.gain = fade.gain_in,
                    _ => {}
                }
            }
        }
        let mut process = self.process();
        process.players = Self::players(
            probe.as_ref(),
            current.as_ref(),
            matches!(what, What::System | What::All),
        );
        let mut app = self.app();
        app.panel_open = super::PANEL.with_borrow(|panel| {
            panel
                .as_ref()
                .is_some_and(|panel| panel.window().is_visible())
        });

        Snapshot {
            schema: SCHEMA,
            written: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
            uptime: logs::uptime(),
            app,
            process,
            main_loop: Self::main_loop(full),
            dj: Dj {
                on: probe.is_some(),
                settings: Some(settings),
                seed: probe.as_ref().map(|dj| dj.seed.clone()),
                source: probe.as_ref().map(|dj| dj.source.clone()),
                queued: probe.as_ref().map_or(0, |dj| dj.queued),
                played: probe.as_ref().map_or(0, |dj| dj.played),
                starting: probe.as_ref().is_some_and(|dj| dj.starting),
                refilling: probe.as_ref().is_some_and(|dj| dj.refilling),
                hovered: probe.as_ref().and_then(|dj| dj.hovered.clone()),
                warm: probe.as_ref().map_or(Vec::new(), |dj| {
                    dj.warm
                        .iter()
                        .map(|warm| WarmInfo {
                            id: warm.id.clone(),
                            ready: warm.ready,
                            since: warm.since,
                        })
                        .collect()
                }),
            },
            decks,
            fade,
            queue,
            video: current
                .as_ref()
                .filter(|_| listed)
                .and_then(|page| Self::video(page, &client)),
            notes,
            panel: super::COST.get(),
            log: if full {
                logs::tail(DUMP_LOG)
                    .into_iter()
                    .map(|line| LogLine {
                        at: line.at,
                        level: line.level.to_string(),
                        target: line.target,
                        text: line.text,
                    })
                    .collect()
            } else {
                Vec::new()
            },
        }
    }

    /// Give every deck its letter: a track stays where it was, a new one
    /// takes the free deck.
    fn seat(&mut self, decks: &mut Vec<Deck>) {
        let mut seats: [Option<usize>; 2] = [None, None];
        for (index, deck) in decks.iter().enumerate().take(2) {
            if let Some(seat) = self
                .slots
                .iter()
                .position(|slot| slot.as_deref() == Some(deck.id.as_str()))
                && seats[seat].is_none()
            {
                seats[seat] = Some(index);
            }
        }
        for index in 0..decks.len().min(2) {
            if !seats.contains(&Some(index))
                && let Some(free) = seats.iter().position(Option::is_none)
            {
                seats[free] = Some(index);
            }
        }
        let mut seated = Vec::with_capacity(2);
        for (seat, letter) in ["A", "B"].into_iter().enumerate() {
            self.slots[seat] = seats[seat].map(|index| decks[index].id.clone());
            if let Some(index) = seats[seat] {
                let mut deck = std::mem::take(&mut decks[index]);
                deck.deck = letter.to_owned();
                seated.push(deck);
            }
        }
        *decks = seated;
    }

    fn beats_of(&mut self, client: &Client, id: &str) -> Option<TrackBeats> {
        if self.beats.len() > 64 {
            self.beats.retain(|_, (at, _)| at.elapsed() < SLOW * 5);
        }
        match self.beats.get(id) {
            Some((at, known)) if at.elapsed() < SLOW => *known,
            _ => {
                let known = client.core().beats(&video_id(id));
                self.beats.insert(id.to_owned(), (Instant::now(), known));
                known
            }
        }
    }

    fn beats_info(
        &mut self,
        id: &str,
        client: &Client,
        settings: &DjSettings,
        analysed: &[String],
        notes: &logs::Notes,
    ) -> BeatsInfo {
        let known = self.beats_of(client, id);
        let asked = analysed
            .iter()
            .any(|key| key.strip_suffix("+entry").unwrap_or(key) == id);
        let error = notes.analysis_failed.get(id).cloned();
        let status = match known {
            Some(TrackBeats { intro: Some(_), .. }) => "cached",
            Some(_) => "no beat",
            None if error.is_some() => "failed",
            None if asked => "analysing",
            None if settings.match_beats || settings.start_on_beat => "pending",
            None => "off",
        };
        let intro = known.and_then(|known| known.intro);
        BeatsInfo {
            status: status.to_owned(),
            bpm: intro.map(|grid| grid.bpm),
            first_beat: intro.map(|grid| grid.anchor),
            confidence: intro.map(|grid| grid.confidence),
            usable: intro.is_some_and(|grid| grid.is_usable()),
            analysed_seconds: known.map(|known| known.analysed),
            entry: known.and_then(|known| known.entry).map(|entry| EntryInfo {
                start: entry.start,
                found: entry.found.to_string(),
                bpm: entry.grid.map(|grid| grid.bpm),
                confidence: entry.grid.map(|grid| grid.confidence),
            }),
            error,
        }
    }

    /// How ready a queued track is.
    fn readiness(track: &Track, dj: &DjProbe, client: &Client) -> &'static str {
        if let Some(warm) = dj.warm.iter().find(|warm| warm.id == track.id) {
            return if warm.ready { "warm" } else { "warming" };
        }
        if prepared_uri(client, &track.id).is_some() {
            "prepared"
        } else if dj.prepared.contains(&track.id) {
            "preparing"
        } else {
            "cold"
        }
    }

    /// The next track, which has no player yet.
    fn queued_deck(
        &mut self,
        track: &Track,
        dj: &DjProbe,
        client: &Client,
        settings: &DjSettings,
        notes: &logs::Notes,
    ) -> Deck {
        let uri = prepared_uri(client, &track.id);
        let beats = self.beats_info(&track.id, client, settings, &dj.analysed, notes);
        Deck {
            role: "queued".into(),
            id: track.id.clone(),
            title: track.title.clone(),
            artist: artist_of(track),
            status: Self::readiness(track, dj, client).to_owned(),
            player: "none".into(),
            duration: track.duration.map(|d| d.as_secs_f64()),
            gain: 1.0,
            tempo: 1.0,
            stream: Stream {
                kind: if uri.is_some() { "prepared" } else { "none" }.into(),
                file_bytes: uri.as_deref().and_then(file_bytes),
                uri,
                audio_only: !settings.show_video,
                ..Stream::default()
            },
            grid: Self::grid(self.beats_of(client, &track.id), None),
            beats,
            waveform: Self::waveform(&track.id),
            ..Deck::default()
        }
    }

    fn grid(known: Option<TrackBeats>, entry: Option<f64>) -> Option<Grid> {
        let known = known?;
        let at = entry.unwrap_or(0.0);
        let from_entry = known
            .entry
            .is_some_and(|found| found.start > 0.0 && (at - found.start).abs() < 1.0);
        known.grid_at(at).map(|grid| Grid {
            bpm: grid.bpm,
            anchor: grid.anchor,
            confidence: grid.confidence,
            from: if from_entry { "entry" } else { "intro" }.into(),
        })
    }

    fn waveform(id: &str) -> Waveform {
        peaks::with(id, |track| match track {
            Some(track) => Waveform {
                known_seconds: track.known_seconds(),
                source: track.decode.label(),
            },
            None => Waveform {
                known_seconds: 0.0,
                source: "none".into(),
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn deck(
        &mut self,
        page: &VideoPage,
        role: &'static str,
        grid: Option<(BeatGrid, &'static str)>,
        warm: Option<WarmInfo>,
        client: &Client,
        settings: &DjSettings,
        analysed: &[String],
        notes: &logs::Notes,
        waves: bool,
    ) -> Option<Deck> {
        let video = page.video()?;
        let id = video.id().id;
        let probe = page.dev_probe();
        let player = page.clapper_player();
        let state = page.player_state();
        let position = page.heard_position();
        let duration = page.duration_secs();
        let status = if probe.scratching {
            "scratching"
        } else if probe.warming && probe.warm_held {
            "held"
        } else if probe.warming && page.is_warm() {
            "warm"
        } else if probe.warming {
            "warming"
        } else {
            state_name(state)
        };
        let volume = player.as_ref().map_or(0.0, |player| player.volume());
        let gain = match probe.fade_base {
            Some(base) if base > 0.0 => (volume / base).powi(3),
            Some(_) => 0.0,
            None => 1.0,
        };

        let item_uri = player
            .as_ref()
            .and_then(|player| player.queue())
            .and_then(|queue| queue.current_item())
            .and_then(|item| item.uri())
            .map(|uri| uri.to_string());
        let prepared = page.playing_uri();
        let manifest = |uri: &str| {
            if uri.contains(".mpd") { "DASH" } else { "HLS" }
        };
        let kind = match (&prepared, &item_uri) {
            _ if video.downloaded() => "downloaded file".to_owned(),
            (Some(uri), _) if probe.audio_stream => format!("prepared {} audio", manifest(uri)),
            (Some(uri), _) => format!("prepared {}", manifest(uri)),
            (None, Some(_)) if probe.fast_start => "fast start".to_owned(),
            (None, Some(_)) => "yt-dlp fallback".to_owned(),
            (None, None) => "none".to_owned(),
        };
        let key = page.as_ptr() as usize;
        if self.streams.len() > 16 {
            self.streams.retain(|_, (at, _)| at.elapsed() < SLOW * 10);
        }
        let remembered = self
            .streams
            .get(&key)
            .filter(|(at, stream)| at.elapsed() < SLOW && stream.kind == kind)
            .map(|(_, stream)| stream.clone());
        let audio = player
            .as_ref()
            .filter(|_| remembered.is_none())
            .and_then(|player| player.audio_streams())
            .and_then(|streams| streams.current_stream())
            .and_then(|stream| stream.downcast::<clapper::AudioStream>().ok());
        let stream = Stream {
            kind,
            file_bytes: item_uri.as_deref().and_then(file_bytes),
            uri: item_uri,
            audio_only: page.audio_only(),
            codec: audio
                .as_ref()
                .and_then(|audio| audio.codec())
                .map(|codec| codec.to_string()),
            sample_rate: audio
                .as_ref()
                .map(|audio| audio.sample_rate().max(0) as u32)
                .filter(|rate| *rate > 0)
                .unwrap_or(probe.format.0),
            channels: audio
                .as_ref()
                .map(|audio| audio.channels().max(0) as u32)
                .filter(|channels| *channels > 0)
                .unwrap_or(probe.format.1),
            bitrate: audio.as_ref().map_or(0, |audio| audio.bitrate()),
        };
        let stream = match remembered {
            Some(remembered) => remembered,
            None => {
                self.streams.insert(key, (Instant::now(), stream.clone()));
                stream
            }
        };

        if waves {
            peaks::live_restart(&id, position);
            peaks::feed_live(&id, |from, sink| page.dev_read_audio(from, sink));
            let uri = prepared
                .clone()
                .or_else(|| client.prepared_audio_playback_uri(&video))
                .or_else(|| client.prepared_playback_uri(&video));
            if let Some(uri) = uri {
                peaks::ensure_decoded(&id, &uri, duration);
            }
        }

        let known = self.beats_of(client, &id);
        let grid = match grid {
            Some((grid, from)) => Some(Grid {
                bpm: grid.bpm,
                anchor: grid.anchor,
                confidence: grid.confidence,
                from: from.into(),
            }),
            None => Self::grid(known, page.entry()),
        };
        Some(Deck {
            deck: String::new(),
            role: role.into(),
            title: video.title(),
            artist: video.uploader().name(),
            status: status.into(),
            player: state_name(state).into(),
            loaded: duration.is_some(),
            position,
            duration,
            buffered_ahead: self.buffered(page, probe.pipeline.as_ref(), position),
            decoded_to: page.flow_position(),
            volume,
            base_volume: probe.fade_base,
            gain,
            tempo: page.tempo(),
            can_stretch: page.can_stretch(),
            stream,
            beats: self.beats_info(&id, client, settings, analysed, notes),
            grid,
            entry_target: page.entry(),
            entry_ready: page.entry().is_some() && page.entry_ready(),
            warm,
            waveform: Self::waveform(&id),
            id,
        })
    }

    /// Seconds downloaded past `position`: the adaptive demuxer's level
    /// (HLS, DASH), else the pipeline's buffering ranges.
    fn buffered(
        &mut self,
        page: &VideoPage,
        pipeline: Option<&gst::Element>,
        position: f64,
    ) -> Option<f64> {
        use gst::prelude::*;
        let key = page.as_ptr() as usize;
        if self.buffer.len() > 16 {
            self.buffer.retain(|_, (at, ..)| at.elapsed() < SLOW * 10);
        }
        let demux = match self.buffer.get(&key) {
            Some((at, ahead, _)) if at.elapsed() < BUFFER_EVERY => return *ahead,
            Some((_, _, demux)) => demux.as_ref().and_then(|demux| demux.upgrade()),
            None => None,
        };
        let pipeline = pipeline?;
        let demux = demux.filter(|demux| demux.parent().is_some()).or_else(|| {
            pipeline
                .downcast_ref::<gst::Bin>()?
                .iterate_recurse()
                .into_iter()
                .flatten()
                .find(|element| {
                    element
                        .factory()
                        .is_some_and(|f| matches!(f.name().as_str(), "hlsdemux2" | "dashdemux2"))
                })
        });
        let ahead = match demux.as_ref() {
            Some(demux) => {
                let level = |name: &str| match demux.property::<u64>(name) {
                    u64::MAX => 0.0,
                    ns => ns as f64 / 1e9,
                };
                let (video, audio) = (
                    level("current-level-time-video"),
                    level("current-level-time-audio"),
                );
                Some(match (video > 0.0, audio > 0.0) {
                    (true, true) => video.min(audio),
                    (true, false) => video,
                    _ => audio,
                })
            }
            None => {
                let mut query = gst::query::Buffering::new(gst::Format::Time);
                pipeline
                    .query(&mut query)
                    .then(|| {
                        let at = (position * 1e9) as i64;
                        query.ranges().into_iter().find_map(|(start, stop)| {
                            let (start, stop) = (start.value(), stop.value());
                            (start <= at && stop > at).then(|| (stop - at) as f64 / 1e9)
                        })
                    })
                    .flatten()
            }
        };
        self.buffer.insert(
            key,
            (
                Instant::now(),
                ahead,
                demux.as_ref().map(|demux| demux.downgrade()),
            ),
        );
        ahead
    }

    fn fade(probe: Option<&DjProbe>, full: bool) -> Option<Fade> {
        let live = probe.and_then(|dj| dj.fade.as_ref());
        super::FADE.with_borrow(|log| {
            let last = log.samples.last().copied();
            // (While a nudge is heard nothing is read: the reading before it stands.)
            let reading = log
                .samples
                .iter()
                .rev()
                .find_map(|sample| Some((sample.beat_ms?, sample.settled)));
            let id = |page: &VideoPage| page.video().map(|video| video.id().id);
            match live {
                Some(fade) => {
                    let (elapsed, total) = fade.running.unwrap_or((0.0, fade.wanted));
                    // Before both play, the last fade's samples are still there.
                    let last = last.filter(|_| fade.running.is_some());
                    Some(Fade {
                        live: true,
                        phase: if fade.running.is_some() {
                            "fading"
                        } else {
                            "starting"
                        }
                        .into(),
                        outgoing: id(&fade.outgoing),
                        incoming: id(&fade.incoming),
                        wanted: Some(fade.wanted),
                        waited: Some(fade.since),
                        elapsed,
                        total,
                        gain_out: last.map_or(1.0, |s| s.out as f64),
                        gain_in: last.map_or(0.0, |s| s.inc as f64),
                        readiness: Some(fade.readiness.into()),
                        to_entry: Some(fade.entry.into()),
                        sync: fade.sync.as_ref().map(|sync| Sync {
                            stage: sync.stage.into(),
                            bpm_in: sync.incoming.bpm,
                            bpm_out: sync.outgoing.map(|grid| grid.bpm),
                            stretch_percent: sync.matched.map(|m| (m.rate - 1.0) * 100.0),
                            multiple: sync.matched.map(|m| m.multiple),
                            let_go: sync.let_go,
                            errors_ms: sync.errors.iter().map(|e| e * 1000.0).collect(),
                            nudging: sync.nudging,
                        }),
                        beat_ms: reading
                            .filter(|_| fade.running.is_some())
                            .map(|(ms, _)| ms as f64),
                        beat_settled: reading.is_some_and(|(_, settled)| settled),
                        samples: if full && fade.running.is_some() {
                            log.samples.clone()
                        } else {
                            Vec::new()
                        },
                    })
                }
                None => last.map(|last| Fade {
                    live: false,
                    phase: "done".into(),
                    elapsed: last.t as f64,
                    total: log.total,
                    gain_out: last.out as f64,
                    gain_in: last.inc as f64,
                    beat_ms: reading.map(|(ms, _)| ms as f64),
                    beat_settled: reading.is_some_and(|(_, settled)| settled),
                    samples: if full {
                        log.samples.clone()
                    } else {
                        Vec::new()
                    },
                    ..Fade::default()
                }),
            }
        })
    }

    /// Every video page that is alive; `detailed`: with what it is doing
    /// (else only how many there are).
    fn players(
        probe: Option<&DjProbe>,
        current: Option<&VideoPage>,
        detailed: bool,
    ) -> Vec<PlayerInfo> {
        super::live_pages()
            .iter()
            .map(|page| {
                if !detailed {
                    return PlayerInfo::default();
                }
                let fade = probe.and_then(|dj| dj.fade.as_ref());
                let role = if fade.is_some_and(|fade| fade.outgoing == *page) {
                    "outgoing"
                } else if fade.is_some_and(|fade| fade.incoming == *page) {
                    "incoming"
                } else if current == Some(page) {
                    "current"
                } else if probe.is_some_and(|dj| dj.warm.iter().any(|warm| warm.page == *page)) {
                    "warm"
                } else if page.parked_for().is_some() {
                    "parked"
                } else {
                    "retired"
                };
                PlayerInfo {
                    id: page.video().map(|video| video.id().id).unwrap_or_default(),
                    state: state_name(page.player_state()).into(),
                    role: role.into(),
                    age: page.dev_probe().age,
                }
            })
            .collect()
    }

    fn video(page: &VideoPage, client: &Client) -> Option<VideoMeta> {
        let video: Video = page.video()?;
        let id = video.id();
        Some(VideoMeta {
            platform: id.platform.to_string(),
            title: video.title(),
            uploader: video.uploader().name(),
            uploader_id: video.uploader().id().id,
            duration: video
                .duration()
                .map(|d| d.num_milliseconds() as f64 / 1000.0),
            views: video.has_views().then(|| video.views()),
            likes: video.has_likes().then(|| video.likes()),
            live: video.is_live(),
            uploaded: video.uploaded().to_rfc3339(),
            downloaded: video.downloaded(),
            dj_page: page.dj(),
            audio_only: page.audio_only(),
            prepared: client.prepared_playback_uri(&video).is_some(),
            prepared_audio: client.prepared_audio_playback_uri(&video).is_some(),
            id: id.id,
        })
    }

    fn main_loop(full: bool) -> MainLoop {
        super::HEART.with_borrow(|heart| {
            let count = heart.lags.len().max(1) as f64;
            MainLoop {
                lag_ms_mean: heart.lags.iter().map(|(_, lag)| *lag as f64).sum::<f64>() / count,
                lag_ms_max: heart
                    .lags
                    .iter()
                    .fold(0.0, |max, (_, lag)| f64::max(max, *lag as f64)),
                stalls: heart.stalls,
                samples: if full {
                    heart.lags.iter().copied().collect()
                } else {
                    Vec::new()
                },
            }
        })
    }

    fn app(&mut self) -> App {
        self.app
            .get_or_insert_with(|| App {
                version: env!("CARGO_PKG_VERSION").into(),
                app_id: crate::config::APP_ID.into(),
                profile: crate::config::PROFILE.into(),
                pid: std::process::id(),
                gtk: format!(
                    "{}.{}.{}",
                    gtk::major_version(),
                    gtk::minor_version(),
                    gtk::micro_version()
                ),
                gstreamer: gst::version_string().to_string(),
                panel_open: false,
                env: std::env::vars()
                    .filter(|(key, _)| key.starts_with("KH_") || key == "RUST_LOG")
                    .collect(),
            })
            .clone()
    }

    /// What `/proc` says (Linux; elsewhere it stays empty), once a second.
    fn process(&mut self) -> Process {
        if let Some((at, process)) = self.process.as_ref()
            && at.elapsed() < SLOW
        {
            return process.clone();
        }
        let mut process = Process::default();
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            let field = |name: &str| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix(name))
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|value| value.parse::<f64>().ok())
            };
            process.rss_mb = field("VmRSS:").unwrap_or(0.0) / 1024.0;
            process.peak_rss_mb = field("VmHWM:").unwrap_or(0.0) / 1024.0;
            process.threads = field("Threads:").unwrap_or(0.0) as u32;
        }
        // utime + stime, in ticks of 1/100 s: the 14th and 15th fields,
        // counted after the name (which may hold spaces).
        let ticks = std::fs::read_to_string("/proc/self/stat")
            .ok()
            .and_then(|stat| {
                let rest = stat.rsplit_once(')')?.1.to_owned();
                let mut fields = rest.split_whitespace().skip(11);
                Some(fields.next()?.parse::<u64>().ok()? + fields.next()?.parse::<u64>().ok()?)
            });
        if let Some(ticks) = ticks {
            if let Some((at, before)) = self.cpu {
                let seconds = at.elapsed().as_secs_f64().max(0.001);
                process.cpu_percent = ticks.saturating_sub(before) as f64 / seconds;
            }
            self.cpu = Some((Instant::now(), ticks));
        }
        process.open_files = std::fs::read_dir("/proc/self/fd").map_or(0, |dir| dir.count() as u32);
        let mut names: BTreeMap<String, u32> = BTreeMap::new();
        if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
            for task in tasks.flatten() {
                if let Ok(name) = std::fs::read_to_string(task.path().join("comm")) {
                    let name = name.trim();
                    let name = name.split_once(':').map_or(name, |(head, _)| head);
                    let name = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-');
                    *names.entry(name.to_owned()).or_default() += 1;
                }
            }
        }
        let mut names: Vec<(String, u32)> = names.into_iter().collect();
        names.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        process.thread_names = names;
        self.process = Some((Instant::now(), process.clone()));
        process
    }
}

/// The size of the local file behind a `file://` URI, if it is there.
fn file_bytes(uri: &str) -> Option<u64> {
    let path = url::Url::parse(uri).ok()?.to_file_path().ok()?;
    std::fs::metadata(path).ok().map(|meta| meta.len())
}

impl Collector {
    /// Where the tracks on deck A and deck B are heard right now (between
    /// two snapshots, for the waveforms).
    pub fn positions(&self) -> [Option<f64>; 2] {
        [0, 1].map(|seat| {
            self.pages[seat]
                .as_ref()
                .and_then(|page| page.upgrade())
                .map(|page| page.heard_position())
        })
    }
}
