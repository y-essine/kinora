//! Kinora: the DJ (see `docs/design.md`, "DJ", and `docs/architecture.md`,
//! "The DJ"). The engine is `kinora_core::dj`; this is its frontend.
//!
//! The DJ is a mode of the one player, not another place: every track is a
//! video page in the player sheet, which then shows the DJ's own layout (a
//! record player, large controls, the queue underneath) instead of the video
//! page's. [`Dj`] keeps the session on the UI thread and runs its loop:
//!
//! - while a track plays, the next one is prepared and the transition is
//!   planned from the player's real length;
//! - shortly before the planned start, the next track's page is opened *over*
//!   the current one, which keeps playing without its picture. Once the new
//!   player really plays, both volumes follow the plan's equal-power curve
//!   every 50 ms; then the old player is paused and let go;
//! - a cut (short or live tracks, crossfade off) waits for the end of stream;
//! - Next and Previous are the same crossfade, asked for by hand: it starts as
//!   soon as the other track plays, over the crossfade setting;
//! - the queue is topped up in the background.
//!
//! With "Match Beats" on, a crossfade is also beat-matched (see
//! `docs/architecture.md`, "Beat matching"): the next track's tempo and
//! beats are worked out ahead of time, from the first seconds of its
//! prepared stream; when the fade starts, those of the track that ends are
//! read from the audio it just played. The incoming track is then held for
//! less than a beat and let go so that its beats land on the outgoing ones,
//! at the outgoing tempo, and eases back to its own afterwards.
//!
//! The DJ is audio first: unless "Show video" is on, a track is prepared and
//! played as audio only, and its page shows the artwork. To make changing
//! track immediate, the next track (and the queue row the pointer rests on)
//! is *warmed*: its page is built ahead of time and its player loaded and
//! held paused at the start, at most [`MAX_WARM`] of them.
//!
//! Why this is not a `kinora_core::player::Player` adapter with
//! `crossfade_to`: a track here is a whole page (its own Clapper player with
//! prepared-stream fallbacks, title, details), and the crossfade is between
//! two pages. An adapter over one `clapper::Player` could not own that.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use glib::subclass::types::ObjectSubclassIsExt;
use gtk::{gdk, glib};
use kinora_core::beats::{self, BeatGrid, BeatMatch};
use kinora_core::dj::{DjSeed, DjSession, RadioSource, Track, TransitionPlan};

use crate::backend::{Client, Video};
use crate::gspawn;
use crate::gui::video_page::VideoPage;
use crate::gui::window::Window;

/// How often the loop looks at the player.
const TICK: Duration = Duration::from_millis(200);
/// How often the volumes are set during a fade.
const FADE_STEP: Duration = Duration::from_millis(50);
/// The next player is opened this long before the planned start: about what
/// a prepared stream needs to begin playing.
const LEAD: Duration = Duration::from_millis(2000);
/// "Previous" within this many seconds of a track's start goes back one
/// track; later it starts the track over.
const RESTART_AFTER: f64 = 3.0;
/// Give up waiting for the incoming player after this long.
const START_TIMEOUT: Duration = Duration::from_secs(40);
/// After a failed request for more tracks, wait before asking again.
const REFILL_PAUSE: Duration = Duration::from_secs(15);
/// With the next player warm, it starts at once: open it this long ahead.
const WARM_LEAD: Duration = Duration::from_millis(300);
/// How many players are kept loaded ahead: the next track, and the one the
/// pointer rests on in the queue. (Preparing is capped at two at a time too.)
const MAX_WARM: usize = 2;
/// The pointer rests this long on a queue row before the row is got ready.
const HOVER_DELAY: Duration = Duration::from_millis(400);
/// How many queued tracks the panel lists.
const SHOWN: usize = 30;

// ---- Start on the Beat ----

/// A warm track waits this long for its entry to be worked out before it
/// counts as ready (at its start).
const ENTRY_WAIT: Duration = Duration::from_secs(12);
/// A track that was not ready ahead of time is given this long to get to
/// its entry before the fade starts wherever it is.
const ENTRY_SEEK_TIMEOUT: Duration = Duration::from_millis(2500);

/// Getting the incoming track of a fade to its entry.
#[derive(Clone, Copy)]
enum ToEntry {
    /// Nothing to do: it starts at its start, or is at its entry already.
    No,
    /// Seek there once its player is loaded.
    Wanted(f64),
    /// Told to, at this time.
    Seeking(Instant),
}

// ---- Beat matching ----

/// After a matched fade, the track goes back to its own tempo over this long.
const TEMPO_HOME: Duration = Duration::from_secs(8);
/// The fade waits this long, at most, for the beats of the track that ends
/// (they are worked out on another thread when the fade is asked for).
const BEATS_WAIT: Duration = Duration::from_millis(300);
/// The incoming player is given this long to come to rest before it is let
/// go on the beat.
const REST_TIMEOUT: Duration = Duration::from_millis(1500);
/// The incoming track is held at least this long, in seconds.
const SHORTEST_HOLD: f64 = 0.04;
/// From telling a held player to play until it is heard, in seconds (measured:
/// the beats of matched fades came that much late without it).
const START_LATENCY: f64 = 0.002;
/// Beats that are further apart than this are nudged together, in seconds...
const NUDGE_FROM: f64 = 0.012;
/// ...by playing a little faster or slower: by this much at most, and for
/// about this long.
const NUDGE_MOST: f64 = 0.04;
const NUDGE_OVER: f64 = 0.6;
/// The tail of the playing track has to be at least this long to find its
/// beats in, in seconds.
const SHORTEST_TAIL: f64 = 5.0;
/// During a matched fade, the outgoing beats are read again this often from
/// what it played since: the beats that are compared are then never far from
/// audio that was looked at.
const REREAD: Duration = Duration::from_millis(1500);
/// A player's clock is only to be trusted to a few milliseconds once it has
/// played for this long.
const CLOCK_SETTLES: Duration = Duration::from_secs(8);

/// Where a beat-matched fade is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The incoming track has to start, and the outgoing beats be known.
    Waiting,
    /// The incoming player was told to pause, to be let go on the beat.
    Resting(Instant),
    /// It rests; a timer lets it go.
    Held,
    /// Both play, beat on beat.
    Matched,
}

/// A nudge that is under way.
struct Nudge {
    /// When to go back to the matched tempo.
    until: Option<Instant>,
    /// When the nudge has been heard in full, and the beats can be compared
    /// again.
    heard: Instant,
}

/// The beat matching of one fade.
struct BeatSync {
    incoming: BeatGrid,
    /// The outgoing track's beats, on their way from the thread that works
    /// them out, and when they were last asked for.
    arriving: Option<std::sync::mpsc::Receiver<Option<BeatGrid>>>,
    read: Instant,
    outgoing: Option<BeatGrid>,
    matched: Option<BeatMatch>,
    stage: Stage,
    asked: Instant,
    /// The incoming track plays at the matched tempo from here on, in seconds.
    matched_from: f64,
    hold: Option<glib::SourceId>,
    /// When the incoming track was let go.
    let_go: Option<Instant>,
    /// The last readings of how far the beats are apart, and those that
    /// counted (the middle one of every three).
    readings: Vec<f64>,
    errors: Vec<f64>,
    nudge: Option<Nudge>,
}

/// What a fade that waits for its beats does next.
enum Beats {
    Wait,
    /// Fade without matching.
    Plain,
    Match(BeatMatch),
}

impl BeatSync {
    /// The incoming track plays: are the outgoing beats there, and do the
    /// tempos match?
    fn poll(&mut self, outgoing_rate: f64) -> Beats {
        if self.outgoing.is_none() {
            match self.arriving.as_ref().map(|arriving| arriving.try_recv()) {
                Some(Ok(Some(grid))) => {
                    self.outgoing = Some(grid);
                    self.arriving = None;
                }
                Some(Ok(None)) | None => {
                    log::info!("DJ: beat match skipped: the playing track has no clear beat");
                    return Beats::Plain;
                }
                Some(Err(std::sync::mpsc::TryRecvError::Empty))
                    if self.asked.elapsed() < BEATS_WAIT =>
                {
                    return Beats::Wait;
                }
                Some(Err(_)) => {
                    log::info!("DJ: beat match skipped: the playing track's beats came too late");
                    return Beats::Plain;
                }
            }
        }
        let Some(outgoing) = self.outgoing else {
            return Beats::Plain;
        };
        match beats::match_tempo(
            outgoing.bpm,
            outgoing_rate,
            self.incoming.bpm,
            beats::MAX_STRETCH,
        ) {
            Ok(matched) => {
                log::info!(
                    "DJ: beat match: {:.1} BPM into {:.1} BPM{}: stretch {:+.1} %",
                    self.incoming.bpm,
                    outgoing.bpm * outgoing_rate,
                    match matched.multiple {
                        m if m > 1.5 => " (counted double)",
                        m if m < 0.75 => " (counted half)",
                        _ => "",
                    },
                    (matched.rate - 1.0) * 100.0
                );
                self.matched = Some(matched);
                Beats::Match(matched)
            }
            Err(why) => {
                log::info!(
                    "DJ: beat match skipped ({:.1} BPM into {:.1} BPM): {why}",
                    self.incoming.bpm,
                    outgoing.bpm * outgoing_rate
                );
                Beats::Plain
            }
        }
    }

    /// Both tracks play: read how far their beats are apart, and nudge the
    /// incoming one when it is too far. A tempo set now is heard a second
    /// later (what the sound device has taken in plays out first), so a
    /// nudge is one measured dose, and the beats are compared again only
    /// once it has been heard.
    fn follow(&mut self, outgoing: &VideoPage, incoming: &VideoPage) -> Option<(f64, bool)> {
        self.read_again(outgoing, incoming);
        let (matched, grid) = (self.matched?, self.outgoing?);
        let now = Instant::now();
        if log::log_enabled!(log::Level::Debug) {
            let (at_out, at_in) = (outgoing.heard_position(), incoming.heard_position());
            log::debug!(
                "DJ: beats: at {:.3} s: out {:.3} s  in {:.3} s (taken in up to {:.3} s)  {:+.1} ms  tempo {:.4}{}",
                self.asked.elapsed().as_secs_f64(),
                at_out,
                at_in,
                incoming.flow_position().unwrap_or(0.0),
                matched.error(&grid, at_out, &self.incoming, at_in) * 1000.0,
                incoming.tempo(),
                if self.nudge.is_some() { "  nudging" } else { "" }
            );
        }
        if let Some(nudge) = self.nudge.as_mut() {
            if nudge.until.is_some_and(|until| now >= until) {
                nudge.until = None;
                incoming.set_tempo(matched.rate);
            }
            if now < nudge.heard {
                return None;
            }
            self.nudge = None;
            self.readings.clear();
        }
        let playing = |page: &VideoPage| page.player_state() == clapper::PlayerState::Playing;
        let position = incoming.heard_position();
        if !playing(outgoing) || !playing(incoming) || position < self.matched_from + 0.1 {
            return None;
        }
        let error = matched.error(&grid, outgoing.heard_position(), &self.incoming, position);
        // A player's clock wanders in its first seconds (by 70 ms, measured):
        // what it says then is shown, but nothing is done about it. The start
        // was timed from the track's very beginning, which needs no clock.
        let settled = self.let_go.is_some_and(|at| at.elapsed() >= CLOCK_SETTLES)
            || (cfg!(feature = "dev-hooks") && std::env::var("KH_TRUST_CLOCK").is_ok());
        if !settled {
            return Some((error, false));
        }
        self.readings.push(error);
        if self.readings.len() < 3 {
            return Some((error, true));
        }
        self.readings.sort_by(f64::total_cmp);
        let middle = self.readings[1];
        self.readings.clear();
        self.errors.push(middle);
        if middle.abs() >= NUDGE_FROM {
            // Early beats: slower for a moment. `by` of the tempo for
            // `lasts` seconds takes `by * lasts` seconds off the lead.
            let by = (middle / NUDGE_OVER).clamp(-NUDGE_MOST, NUDGE_MOST);
            let lasts = middle / by;
            let ahead = incoming
                .flow_position()
                .map_or(1.2, |flow| ((flow - position) / matched.rate).clamp(0.0, 3.0));
            incoming.set_tempo(matched.rate * (1.0 - by));
            self.nudge = Some(Nudge {
                until: Some(now + Duration::from_secs_f64(lasts)),
                heard: now + Duration::from_secs_f64(lasts + ahead + 0.1),
            });
            log::info!(
                "DJ: beats {:+.0} ms apart: {:+.1} % for {:.2} s",
                middle * 1000.0,
                -by * 100.0,
                lasts
            );
        }
        Some((error, true))
    }

    /// Keep the outgoing beats fresh: read them again from what it played
    /// since, around the tempo found before. A tempo that comes out a little
    /// different is followed.
    fn read_again(&mut self, outgoing: &VideoPage, incoming: &VideoPage) {
        let Some(arriving) = self.arriving.as_ref() else {
            let (Some(grid), true) = (self.outgoing, self.read.elapsed() >= REREAD) else {
                return;
            };
            self.read = Instant::now();
            if let Some(tail) = outgoing.deck_audio() {
                self.arriving = spawn_tail(tail, Some(grid.bpm), false);
            }
            return;
        };
        let grid = match arriving.try_recv() {
            Ok(Some(grid)) => grid,
            Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.arriving = None;
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
        };
        self.arriving = None;
        self.outgoing = Some(grid);
        let Some(matched) = self.matched else {
            return;
        };
        let again = beats::match_tempo(
            grid.bpm,
            matched.rate_out,
            self.incoming.bpm,
            beats::MAX_STRETCH,
        );
        if let Ok(again) = again.map(|again| BeatMatch {
            // (Counted as before: a fade does not change its mind on that.)
            rate: again.rate * again.multiple / matched.multiple,
            multiple: matched.multiple,
            period_in: matched.period_in,
            ..again
        }) && (again.rate - 1.0).abs() <= beats::MAX_STRETCH + 0.01
        {
            if (again.rate - matched.rate).abs() > 0.001 {
                log::info!(
                    "DJ: the playing track is at {:.1} BPM now: stretch {:+.1} %",
                    grid.bpm,
                    (again.rate - 1.0) * 100.0
                );
                if self.nudge.as_ref().is_none_or(|nudge| nudge.until.is_none()) {
                    incoming.set_tempo(again.rate);
                }
            }
            self.matched = Some(again);
        }
    }

    /// For the log: how well the beats were together.
    fn summary(&self) -> String {
        if self.errors.is_empty() {
            return format!(
                "started on the beat; not measured (a player's clock takes {} s to settle)",
                CLOCK_SETTLES.as_secs()
            );
        }
        let n = self.errors.len() as f64;
        let mean = self.errors.iter().map(|e| e.abs()).sum::<f64>() / n;
        let worst = self.errors.iter().fold(0.0f64, |w, e| w.max(e.abs()));
        format!(
            "{} readings, beats {:.0} ms apart on average, {:.0} ms at worst, {:+.0} ms at first, {:+.0} ms at the end",
            self.errors.len(),
            mean * 1000.0,
            worst * 1000.0,
            self.errors[0] * 1000.0,
            self.errors[self.errors.len() - 1] * 1000.0
        )
    }
}

/// Work out the beats of a track's first seconds and keep them (on a thread
/// of its own: it decodes). With `entry` ("Start on the Beat") more of the
/// track is decoded, and where it gets going is looked for too; `length` is
/// the track's, in seconds.
fn analyse_intro(
    core: std::sync::Arc<kinora_core::Core>,
    id: pcore::VideoId,
    uri: String,
    entry: bool,
    length: Option<f64>,
) {
    if core
        .beats(&id)
        .is_some_and(|known| !entry || known.entry.is_some())
    {
        return;
    }
    let started = Instant::now();
    let pcm = match crate::gui::deck_audio::decode(
        &uri,
        0.0,
        if entry {
            beats::ENTRY_WINDOW
        } else {
            beats::INTRO
        },
        beats::ANALYSIS_RATE,
        1,
        Duration::from_secs(30),
        None,
    ) {
        Ok(pcm) => pcm,
        Err(e) => {
            log::warn!("DJ: could not decode {} for its beats: {e}", id.id);
            return;
        }
    };
    let decoded = started.elapsed();
    let analysed = core.analyse_beats(&id, &pcm.samples, pcm.rate, entry, length);
    let took = started.elapsed();
    match analysed.intro {
        Some(grid) => log::info!(
            "DJ: beats of {}: {:.1} BPM, first beat at {:.2} s, confidence {:.2}{} ({:.0} s decoded in {:.2} s, analysed in {:.0} ms)",
            id.id,
            grid.bpm,
            grid.anchor,
            grid.confidence,
            if grid.is_usable() {
                ""
            } else {
                " (too vague to match)"
            },
            pcm.seconds(),
            decoded.as_secs_f64(),
            (took - decoded).as_secs_f64() * 1000.0
        ),
        None => log::info!(
            "DJ: beats of {}: none found ({:.0} s decoded in {:.2} s)",
            id.id,
            pcm.seconds(),
            decoded.as_secs_f64()
        ),
    }
    if let Some(entry) = analysed.entry {
        log::info!(
            "DJ: entry of {}: {} ({}{})",
            id.id,
            if entry.start > 0.0 {
                format!("at {:.2} s", entry.start)
            } else {
                "at its start".to_owned()
            },
            entry.found,
            entry
                .grid
                .filter(|_| entry.start > 0.0)
                .map_or(String::new(), |grid| format!(
                    "; {:.1} BPM there, confidence {:.2}",
                    grid.bpm, grid.confidence
                ))
        );
    }
}

/// The beats of the track that ends, from the last seconds it played:
/// around the tempo of its start when that is known, else (or when that
/// finds nothing clear: tracks change) any tempo.
fn analyse_tail(
    pcm: crate::gui::deck_audio::Pcm,
    near: Option<f64>,
    first: bool,
) -> Option<BeatGrid> {
    let started = Instant::now();
    let (mono, rate) = beats::downmix(&pcm.samples, pcm.channels, pcm.rate, beats::ANALYSIS_RATE);
    let grid = near
        .and_then(|bpm| beats::analyse(&mono, rate, pcm.start, Some(bpm)))
        .filter(BeatGrid::is_usable)
        .or_else(|| beats::analyse(&mono, rate, pcm.start, None));
    match grid {
        Some(_) if !first => {}
        Some(grid) => log::info!(
            "DJ: beats of what plays ({:.1} s up to {:.1} s): {:.1} BPM, confidence {:.2}, in {:.0} ms",
            pcm.seconds(),
            pcm.end(),
            grid.bpm,
            grid.confidence,
            started.elapsed().as_secs_f64() * 1000.0
        ),
        None => log::info!("DJ: beats of what plays: none found"),
    }
    grid.filter(BeatGrid::is_usable)
}

/// [`analyse_tail`] on a thread of its own; the answer arrives on the channel.
fn spawn_tail(
    tail: crate::gui::deck_audio::Pcm,
    near: Option<f64>,
    first: bool,
) -> Option<std::sync::mpsc::Receiver<Option<BeatGrid>>> {
    let (send, arriving) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("kinora-beats".into())
        .spawn(move || {
            let _ = send.send(analyse_tail(tail, near, first));
        })
        .ok()
        .map(|_| arriving)
}

/// A fade that is running.
struct Fade {
    /// The page that is ending. It is no longer in the window.
    outgoing: VideoPage,
    /// The page that fades in.
    incoming: VideoPage,
    /// How long the fade should take.
    wanted: Duration,
    began: Instant,
    /// Set once the incoming player plays: the curve, and when it started.
    running: Option<(TransitionPlan, Instant)>,
    steps: u32,
    timer: glib::SourceId,
    /// How ready the incoming track was: "warm", "prepared" or "cold".
    readiness: &'static str,
    /// The fade is beat-matched ("Match Beats"), as far as it has got.
    sync: Option<BeatSync>,
    /// The incoming track still has to get to its entry ("Start on the
    /// Beat": one that was not ready ahead of time).
    entry: ToEntry,
}

/// A track loaded ahead of time: its page, with the player paused at the start.
struct Warm {
    id: String,
    page: VideoPage,
    began: Instant,
    /// Loaded (and said so in the log).
    ready: bool,
}

struct Inner {
    window: glib::WeakRef<Window>,
    session: RefCell<Option<DjSession>>,
    panel: Panel,
    /// The page of the current track.
    page: RefCell<Option<VideoPage>>,
    fade: RefCell<Option<Fade>>,
    tick: RefCell<Option<glib::SourceId>>,
    /// The ids of the tracks whose streams were asked for ahead of time.
    prepared: RefCell<Vec<String>>,
    /// The ids of the tracks whose beats were asked for.
    analysed: RefCell<Vec<String>>,
    /// Players loaded ahead (see [`Warm`]), oldest first.
    warm: RefCell<Vec<Warm>>,
    /// The queued track the pointer rested on last.
    hovered: RefCell<Option<String>>,
    starting: Cell<bool>,
    refilling: Cell<bool>,
    refill_after: Cell<Option<Instant>>,
    /// Counts sessions, so answers for an old one are dropped.
    generation: Cell<u32>,
}

/// The DJ of one window.
#[derive(Clone)]
pub struct Dj(Rc<Inner>);

impl Dj {
    pub fn new(window: &Window) -> Self {
        let dj = Self(Rc::new(Inner {
            window: window.downgrade(),
            session: RefCell::default(),
            panel: Panel::new(),
            page: RefCell::default(),
            fade: RefCell::default(),
            tick: RefCell::default(),
            prepared: RefCell::default(),
            analysed: RefCell::default(),
            warm: RefCell::default(),
            hovered: RefCell::default(),
            starting: Cell::default(),
            refilling: Cell::default(),
            refill_after: Cell::default(),
            generation: Cell::default(),
        }));
        let weak = Rc::downgrade(&dj.0);
        dj.0.panel.connect(move |action| {
            if let Some(inner) = weak.upgrade() {
                Dj(inner).panel_action(action);
            }
        });
        dj
    }

    pub fn is_on(&self) -> bool {
        self.0.session.borrow().is_some()
    }

    fn client(&self) -> Option<Client> {
        self.0.window.upgrade().map(|w| w.client())
    }

    fn toast(&self, text: &str) {
        if let Some(window) = self.0.window.upgrade() {
            window.show_toast(adw::Toast::new(text));
        }
    }

    /// Start the DJ: from the video in the player (following the "start
    /// from" setting), or with `from_mix` from the account's mix.
    pub fn start(&self, from_mix: bool) {
        self.start_from(from_mix.then_some(DjSeed::MyMix));
    }

    /// Play a playlist or a mix with the DJ: all of it, in order, each item
    /// fading into the next.
    pub fn start_playlist(&self, id: &str, title: &str) {
        match playlist_seed(id, title) {
            Some(seed) => self.start_from(Some(seed)),
            None => {
                log::info!("DJ: no playlist seed in the engine yet ({id}, {title})");
                self.toast(&gettextrs::gettext("The DJ cannot play playlists yet"));
            }
        }
    }

    /// Start the DJ from `seed` (the mix, a playlist, ...); `None`: from the
    /// video in the player, following the "start from" setting.
    pub fn start_from(&self, seed: Option<DjSeed>) {
        let Some(window) = self.0.window.upgrade() else {
            return;
        };
        if self.0.starting.get() {
            return;
        }
        if self.is_on() {
            // Asked for something in particular: that replaces what plays.
            if seed.is_none() {
                window.imp().expand_player();
                return;
            }
            self.leave();
        }
        let client = window.client();
        if !client.dj_enabled() {
            self.toast(&gettextrs::gettext(
                "The DJ is switched off. Turn it on in Preferences, Extensions",
            ));
            return;
        }
        let video = window
            .imp()
            .current_video_page()
            .and_then(|p| p.video())
            .filter(|v| v.platform() == "youtube");
        let seed = seed.unwrap_or_else(|| client.dj_seed(video.as_ref()));
        log::info!("DJ: starting from {seed:?}");
        self.0.starting.set(true);
        let dj = self.clone();
        let generation = self.0.generation.get();
        gspawn!(async move {
            let started = Instant::now();
            let result = client.dj_start(seed.clone()).await;
            dj.0.starting.set(false);
            if dj.0.generation.get() != generation || dj.is_on() {
                return;
            }
            match result {
                Ok(session) => {
                    log::info!(
                        "DJ: {} tracks from {:?} in {:.1} s (personal: {})",
                        session.upcoming().len(),
                        session.source(),
                        started.elapsed().as_secs_f64(),
                        session.personal()
                    );
                    dj.begin(session, seed);
                }
                Err(e) => {
                    log::warn!("DJ: could not start: {e}");
                    dj.toast(&gettextrs::gettext("The DJ could not find music to play"));
                }
            }
        });
    }

    /// A session arrived: take over the playing video, or open the first track.
    fn begin(&self, mut session: DjSession, seed: DjSeed) {
        let Some(window) = self.0.window.upgrade() else {
            return;
        };
        // A video's radio takes over the video that plays; every other seed
        // (the mix, a playlist) starts with its own first track.
        let taken_over = seed_video(&seed).and_then(|id| {
            window
                .imp()
                .current_video_page()
                .and_then(|p| p.video().map(|v| (p, v)))
                .filter(|(_, v)| v.id().id == id)
        });
        if let Some(id) = seed_video(&seed) {
            let Some((page, video)) = taken_over else {
                log::info!("DJ: the video is gone; not starting");
                return;
            };
            if session.current().is_none_or(|t| t.id != id) {
                session.play_next(Track::from_video(&video.video()));
                session.advance();
            }
            self.0.session.replace(Some(session));
            page.set_dj(true);
            self.adopt(&page);
        } else {
            let first = match session.current() {
                Some(track) => Some(track.clone()),
                None => session.advance().cloned(),
            };
            let Some(track) = first else {
                self.toast(&gettextrs::gettext("The DJ could not find music to play"));
                return;
            };
            self.0.session.replace(Some(session));
            let video = window.client().dj_video(&track);
            let (page, _) = window.imp().open_dj_track(&video, false, None);
            self.adopt(&page);
        }
        window.set_dj_on(true);
        let dj = Rc::downgrade(&self.0);
        let tick = glib::timeout_add_local(TICK, move || match dj.upgrade() {
            Some(inner) => {
                Dj(inner).tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        if let Some(old) = self.0.tick.replace(Some(tick)) {
            old.remove();
        }
        self.refill();
    }

    /// Leave the DJ: what plays goes on as an ordinary video, with Up next
    /// and autoplay as before.
    pub fn leave(&self) {
        let inner = &self.0;
        inner.generation.set(inner.generation.get().wrapping_add(1));
        inner.starting.set(false);
        if inner.session.take().is_none() {
            return;
        }
        log::info!("DJ: off");
        self.finish_fade();
        if let Some(tick) = inner.tick.take() {
            tick.remove();
        }
        self.forget_ahead();
        inner.refilling.set(false);
        inner.refill_after.set(None);
        if let Some(page) = inner.page.take() {
            page.set_dj(false);
        }
        inner.panel.clear();
        if let Some(window) = inner.window.upgrade() {
            window.set_dj_on(false);
        }
    }

    /// Next: crossfade into the next track now.
    pub fn skip(&self) {
        self.step(false);
    }

    /// Previous: start the track over, or, within its first seconds, crossfade
    /// back into the one that played before.
    pub fn previous(&self) {
        let page = self.0.page.borrow().clone();
        match page {
            // (Counted from where the track came in.)
            Some(page)
                if page.position_secs() - page.entry().unwrap_or(0.0) > RESTART_AFTER
                    || !self.can_go_back() =>
            {
                log::info!("DJ: previous: starting the track over");
                page.restart();
            }
            _ => self.step(true),
        }
    }

    /// A video was picked somewhere in the app while the DJ is on. If it is
    /// in the queue (a song of the playlist or mix that plays), the DJ mixes
    /// into it like Next does, and `true` is returned; else it is not the
    /// DJ's to play.
    pub fn play_queued(&self, video: &Video) -> bool {
        let id = video.id().id;
        let queued = self
            .0
            .session
            .borrow_mut()
            .as_mut()
            .is_some_and(|s| s.upcoming().iter().any(|t| t.id == id) && s.move_to(&id, 0));
        if queued {
            log::info!("DJ: picked {id} from the queue");
            self.step(false);
            self.refresh_panel();
        }
        queued
    }

    /// Test hook: back one track, wherever the current one is.
    #[cfg(feature = "dev-hooks")]
    pub fn back(&self) {
        self.step(true);
    }

    /// There is a track to go on to.
    pub fn can_go_next(&self) -> bool {
        self.0
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.peek_next().is_some())
    }

    /// A track played before this one.
    pub fn can_go_back(&self) -> bool {
        self.0
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.can_go_back())
    }

    /// A track was asked for and has not started playing yet: the fade waits
    /// for it, and so do Next and Previous.
    pub fn is_pending(&self) -> bool {
        self.0
            .fade
            .borrow()
            .as_ref()
            .is_some_and(|f| f.running.is_none())
    }

    /// Next or Previous by hand: the other track's page opens over the
    /// current one, and they crossfade over the setting as soon as it plays.
    fn step(&self, back: bool) {
        let inner = &self.0;
        let Some(window) = inner.window.upgrade() else {
            return;
        };
        if !self.is_on() || self.is_pending() {
            return;
        }
        // A fade that is under way ends here: one crossfade at a time.
        self.finish_fade();
        let track = inner.session.borrow_mut().as_mut().and_then(|s| {
            if back {
                s.back().cloned()
            } else {
                s.skip().cloned()
            }
        });
        let Some(track) = track else {
            if !back {
                self.out_of_music();
            }
            return;
        };
        let client = window.client();
        let wanted = client.core().dj_settings().crossfade;
        log::info!(
            "DJ: {} by hand: crossfade to {} ({}) over the {:.0} s setting",
            if back { "previous" } else { "next" },
            track.id,
            track.title,
            wanted.as_secs_f64()
        );
        let (warmed, readiness) = self.take_ahead(&client, &track);
        Self::hold_for_beats(&client, warmed.as_ref());
        let (page, old) = window
            .imp()
            .open_dj_track(&client.dj_video(&track), true, warmed);
        self.adopt(&page);
        if let Some(outgoing) = old {
            // Silent until it really plays; then the curve takes over.
            page.set_fade_gain(Some(0.0));
            self.start_fade(outgoing, page, wanted, readiness);
        } else {
            page.set_fade_gain(None);
            start_held(&page);
        }
    }

    /// Nothing is queued: more may be on its way; else the DJ is done.
    fn out_of_music(&self) {
        let inner = &self.0;
        let finished = inner
            .session
            .borrow()
            .as_ref()
            .is_none_or(|s| s.refill_request().is_none());
        if finished && !inner.refilling.get() {
            self.toast(&gettextrs::gettext("The DJ has run out of music"));
            self.leave();
        }
    }

    /// Tell the panel and the window's Next and Previous what is possible now.
    fn changed(&self) {
        self.0.panel.set_pending(self.is_pending());
        if let Some(window) = self.0.window.upgrade() {
            window.imp().update_transport();
        }
    }

    /// `page` is the current track's page from now on.
    fn adopt(&self, page: &VideoPage) {
        self.0.page.replace(Some(page.clone()));
        page.set_dj_panel(Some(self.0.panel.parts()));
        self.refresh_panel();
    }

    fn refresh_panel(&self) {
        let (Some(client), session) = (self.client(), self.0.session.borrow()) else {
            return;
        };
        if let Some(session) = session.as_ref() {
            self.0.panel.show(session, &client);
        }
        drop(session);
        self.changed();
    }

    // ---- The loop ----

    fn tick(&self) {
        let inner = &self.0;
        let (Some(window), Some(page)) = (inner.window.upgrade(), inner.page.borrow().clone())
        else {
            return;
        };
        // The player shows something else, or is gone: the DJ is over.
        if window.imp().current_video_page().as_ref() != Some(&page) {
            return self.leave();
        }
        if inner.fade.borrow().is_some() {
            return;
        }
        let client = window.client();
        let state = page.player_state();
        let playing = state == clapper::PlayerState::Playing;
        let position = page.position_secs();
        let duration = page.duration_secs();
        let next = inner
            .session
            .borrow()
            .as_ref()
            .and_then(|s| s.peek_next().cloned());

        // Get the next track ready once this one is under way.
        self.warm_up(&client, next.as_ref().filter(|_| playing && position > 3.0));
        // A track that plays unprepared (the first one) gets its audio
        // prepared all the same: that is what the scratch decodes more of
        // the track from.
        if playing
            && position > 3.0
            && page.playing_uri().is_none()
            && let Some(video) = page.video()
        {
            let mut asked = inner.prepared.borrow_mut();
            let id = video.id().id;
            if !asked.contains(&id) {
                asked.push(id);
                if client.prepared_audio_playback_uri(&video).is_none() {
                    client.prepare_audio_playback(&video);
                }
            }
        }
        self.refill();

        // The track ended by itself (a cut, or nothing was ready to fade to).
        let ended = matches!(
            state,
            clapper::PlayerState::Stopped | clapper::PlayerState::Paused
        ) && duration.is_some_and(|d| position >= d - 0.75);
        if ended {
            self.move_on();
            return;
        }

        if let (true, Some(duration), Some(next)) = (playing, duration, &next) {
            let at = Duration::from_secs_f64(position.max(0.0));
            let plan = client.dj_transition(Some(Duration::from_secs_f64(duration)), at, next);
            let lead = if self.is_warm(&next.id) {
                WARM_LEAD
            } else {
                LEAD
            };
            if let Some(start) = plan.start_at
                && !plan.is_cut()
                && at + lead >= start
            {
                self.crossfade(plan.fade);
            }
        }
    }

    /// The track ended by itself: the next one takes its place (a cut).
    fn move_on(&self) {
        let inner = &self.0;
        let Some(window) = inner.window.upgrade() else {
            return;
        };
        let next = inner
            .session
            .borrow_mut()
            .as_mut()
            .and_then(|s| s.advance().cloned());
        let Some(track) = next else {
            return self.out_of_music();
        };
        log::info!("DJ: cut to {} ({})", track.id, track.title);
        let client = window.client();
        let (warmed, readiness) = self.take_ahead(&client, &track);
        log::info!("DJ: {} starts {readiness}", track.id);
        let (page, _) = window
            .imp()
            .open_dj_track(&client.dj_video(&track), false, warmed);
        page.set_fade_gain(None);
        self.adopt(&page);
    }

    /// Open the next track's page over the current one and fade between them.
    fn crossfade(&self, fade: Duration) {
        let inner = &self.0;
        let Some(window) = inner.window.upgrade() else {
            return;
        };
        let next = inner
            .session
            .borrow_mut()
            .as_mut()
            .and_then(|s| s.advance().cloned());
        let Some(track) = next else {
            return;
        };
        log::info!(
            "DJ: crossfade to {} ({}), planned over {:.1} s",
            track.id,
            track.title,
            fade.as_secs_f64()
        );
        let client = window.client();
        let (warmed, readiness) = self.take_ahead(&client, &track);
        Self::hold_for_beats(&client, warmed.as_ref());
        let (page, old) = window
            .imp()
            .open_dj_track(&client.dj_video(&track), true, warmed);
        self.adopt(&page);
        match old {
            Some(outgoing) => {
                // Silent until it really plays; then the curve takes over.
                page.set_fade_gain(Some(0.0));
                self.start_fade(outgoing, page, fade, readiness);
            }
            None => {
                page.set_fade_gain(None);
                start_held(&page);
            }
        }
    }

    fn start_fade(
        &self,
        outgoing: VideoPage,
        incoming: VideoPage,
        wanted: Duration,
        readiness: &'static str,
    ) {
        let dj = Rc::downgrade(&self.0);
        let timer = glib::timeout_add_local(FADE_STEP, move || match dj.upgrade() {
            Some(inner) => Dj(inner).fade_step(),
            None => glib::ControlFlow::Break,
        });
        // The outgoing track stays at the tempo it has now, and out of hands.
        outgoing.hold_tempo();
        outgoing.cancel_scratch();
        let entry = self.entry_for(&incoming);
        let from = match entry {
            ToEntry::Wanted(start) => start,
            _ => incoming.entry().unwrap_or(0.0),
        };
        let sync = self.beat_sync(&outgoing, &incoming, from);
        // (A warm track that has yet to get to its entry waits for that.)
        if sync.is_none() && !matches!(entry, ToEntry::Wanted(_)) {
            start_held(&incoming);
        }
        self.0.fade.replace(Some(Fade {
            outgoing,
            incoming,
            wanted,
            began: Instant::now(),
            running: None,
            steps: 0,
            timer,
            readiness,
            sync,
            entry,
        }));
        self.changed();
    }

    /// With "Start on the Beat" on: where a track that a fade brings in has
    /// yet to be sent. A warm track is at its entry already; one that was
    /// not ready ahead of time gets there when its player is loaded, if its
    /// entry is known (it was analysed before).
    fn entry_for(&self, incoming: &VideoPage) -> ToEntry {
        let Some(client) = self.client() else {
            return ToEntry::No;
        };
        let core = client.core();
        if !core.dj_settings().start_on_beat || incoming.entry().is_some() {
            return ToEntry::No;
        }
        let Some(video) = incoming.video() else {
            return ToEntry::No;
        };
        match core.beats(&video.id()).and_then(|known| known.entry) {
            Some(entry) if entry.start > 0.0 => {
                log::info!(
                    "DJ: {} was not at its entry yet: going to {:.2} s first",
                    video.id().id,
                    entry.start
                );
                ToEntry::Wanted(entry.start)
            }
            Some(_) => ToEntry::No,
            None => {
                log::info!(
                    "DJ: {} starts at its start: its entry is not worked out yet",
                    video.id().id
                );
                ToEntry::No
            }
        }
    }

    /// With "Match Beats" on, a warm track is not started when its page
    /// opens: it waits at its very start, the one place a player is at for
    /// certain, to be let go on the beat (or at once, if the beats do not
    /// match: see `start_held`).
    fn hold_for_beats(client: &Client, warmed: Option<&VideoPage>) {
        if let Some(page) = warmed.filter(|page| page.is_warm())
            && client.core().dj_settings().match_beats
        {
            page.hold_warm(true);
        }
    }

    /// With "Match Beats" on: what a fade from `outgoing` to `incoming`
    /// needs to be beat-matched, or why it will not be (in the log).
    /// The incoming track starts at `from` seconds (its entry, or 0).
    fn beat_sync(&self, outgoing: &VideoPage, incoming: &VideoPage, from: f64) -> Option<BeatSync> {
        let client = self.client()?;
        let core = client.core();
        if !core.dj_settings().match_beats {
            return None;
        }
        let skip = |why: String| {
            log::info!("DJ: beat match skipped: {why}");
            None
        };
        if !incoming.can_stretch() {
            return skip(
                "the next track's player cannot change tempo (loaded before Match Beats \
                 was switched on, or no scaletempo element)"
                    .into(),
            );
        }
        let intro_of = |page: &VideoPage| page.video().and_then(|v| core.beats(&v.id()));
        let grid = match intro_of(incoming) {
            None => return skip("the next track's beats are not worked out yet".into()),
            // (The beats of where it starts: its entry's, or its intro's.)
            Some(known) => match known.grid_at(from) {
                Some(grid) if grid.is_usable() => grid,
                Some(grid) => {
                    return skip(format!(
                        "the next track has no clear beat ({:.1} BPM, confidence {:.2})",
                        grid.bpm, grid.confidence
                    ));
                }
                None => return skip("the next track has no beat".into()),
            },
        };
        let tail = match outgoing.deck_audio() {
            Some(pcm) if pcm.seconds() >= SHORTEST_TAIL => pcm,
            Some(pcm) => {
                return skip(format!(
                    "only {:.1} s of the playing track to go by",
                    pcm.seconds()
                ));
            }
            None => return skip("nothing of the playing track to go by".into()),
        };
        let near = intro_of(outgoing)
            .and_then(|known| known.intro)
            .filter(BeatGrid::is_usable)
            .map(|grid| grid.bpm);
        let Some(arriving) = spawn_tail(tail, near, true) else {
            return skip("no thread to work the beats out on".into());
        };
        Some(BeatSync {
            incoming: grid,
            arriving: Some(arriving),
            read: Instant::now(),
            outgoing: None,
            matched: None,
            stage: Stage::Waiting,
            asked: Instant::now(),
            matched_from: 0.0,
            hold: None,
            let_go: None,
            readings: Vec::new(),
            errors: Vec::new(),
            nudge: None,
        })
    }

    /// The incoming player rests: let it go when its beats will land on the
    /// outgoing ones.
    fn hold_for_the_beat(&self, fade: &mut Fade) {
        let Some(sync) = fade.sync.as_mut() else {
            return;
        };
        let (Some(matched), Some(grid)) = (sync.matched, sync.outgoing) else {
            return;
        };
        let position_in = fade.incoming.heard_position();
        // What the player has taken in plays at the track's own tempo.
        let matched_from = fade
            .incoming
            .flow_position()
            .unwrap_or(position_in)
            .max(position_in);
        // (Stretched, the track will be heard a little off its place.)
        let stretched = BeatGrid {
            anchor: sync.incoming.anchor
                - crate::gui::deck_audio::STRETCH_LAG * (matched.rate - 1.0),
            ..sync.incoming
        };
        let wait = matched.start_delay(
            &grid,
            fade.outgoing.heard_position(),
            &stretched,
            position_in,
            matched_from,
            SHORTEST_HOLD + START_LATENCY,
        ) - START_LATENCY;
        log::info!(
            "DJ: holding the next track {:.0} ms for the beat (at {:.2} s, matched from {:.2} s)",
            wait * 1000.0,
            position_in,
            matched_from
        );
        sync.matched_from = matched_from;
        sync.stage = Stage::Held;
        let dj = Rc::downgrade(&self.0);
        sync.hold = Some(glib::timeout_add_local_once(
            Duration::from_secs_f64(wait.max(0.0)),
            move || {
                if let Some(inner) = dj.upgrade() {
                    Dj(inner).let_go();
                }
            },
        ));
    }

    /// The beat is there: the held track plays, and the fade begins.
    fn let_go(&self) {
        let mut slot = self.0.fade.borrow_mut();
        let Some(fade) = slot.as_mut() else {
            return;
        };
        let Some(sync) = fade.sync.as_mut().filter(|s| s.stage == Stage::Held) else {
            return;
        };
        sync.hold = None;
        sync.stage = Stage::Matched;
        sync.let_go = Some(Instant::now());
        if fade.incoming.is_warm_held() {
            start_held(&fade.incoming);
        } else {
            fade.incoming.resume();
        }
        begin_fade(fade);
        drop(slot);
        self.changed();
    }

    fn fade_step(&self) -> glib::ControlFlow {
        let mut slot = self.0.fade.borrow_mut();
        let Some(fade) = slot.as_mut() else {
            return glib::ControlFlow::Break;
        };
        if fade.running.is_none() {
            let state = fade.incoming.player_state();
            // "Start on the Beat", for a track that was not ready ahead of
            // time: to its entry first, silent as it still is.
            match fade.entry {
                ToEntry::Wanted(start)
                    if state == clapper::PlayerState::Playing || fade.incoming.is_warm() =>
                {
                    fade.incoming.seek_entry(start);
                    fade.entry = ToEntry::Seeking(Instant::now());
                    return glib::ControlFlow::Continue;
                }
                ToEntry::Seeking(since) => {
                    let there = fade.incoming.entry_ready();
                    if !there && since.elapsed() < ENTRY_SEEK_TIMEOUT {
                        return glib::ControlFlow::Continue;
                    }
                    log::info!(
                        "DJ: the next track {} its entry after {:.2} s",
                        if there { "is at" } else { "did not get to" },
                        since.elapsed().as_secs_f64()
                    );
                    fade.entry = ToEntry::No;
                    // (Held for its beat, it stays; else it plays now.)
                    if fade.sync.is_none() {
                        start_held(&fade.incoming);
                    }
                }
                _ => {}
            }
            // A matched fade: the incoming track is brought to rest, then
            // let go on the beat (which starts the fade).
            match fade.sync.as_ref().map(|sync| sync.stage) {
                Some(Stage::Held) => return glib::ControlFlow::Continue,
                Some(Stage::Resting(_)) if state == clapper::PlayerState::Paused => {
                    self.hold_for_the_beat(fade);
                    return glib::ControlFlow::Continue;
                }
                Some(Stage::Resting(since)) if since.elapsed() < REST_TIMEOUT => {
                    return glib::ControlFlow::Continue;
                }
                Some(Stage::Resting(_)) => {
                    log::info!("DJ: beat match given up: the next track did not come to rest");
                    fade.sync = None;
                    fade.incoming.set_tempo(1.0);
                    fade.incoming.resume();
                    return glib::ControlFlow::Continue;
                }
                _ => {}
            }
            // A warm track that waits at its start: let go on the beat, or
            // at once when the beats do not match.
            if fade.incoming.is_warm_held() {
                let beats = match fade.sync.as_mut() {
                    Some(sync) if fade.incoming.can_stretch() => sync.poll(fade.outgoing.tempo()),
                    Some(_) => {
                        log::info!(
                            "DJ: beat match skipped: the next track's player cannot change tempo"
                        );
                        Beats::Plain
                    }
                    None => Beats::Plain,
                };
                match beats {
                    Beats::Wait => {}
                    Beats::Plain => {
                        fade.sync = None;
                        start_held(&fade.incoming);
                    }
                    Beats::Match(matched) => {
                        fade.incoming.set_tempo(matched.rate);
                        self.hold_for_the_beat(fade);
                    }
                }
                return glib::ControlFlow::Continue;
            }
            let started = state == clapper::PlayerState::Playing;
            if started {
                if let Some(sync) = fade.sync.as_mut() {
                    match sync.poll(fade.outgoing.tempo()) {
                        Beats::Wait => return glib::ControlFlow::Continue,
                        Beats::Plain => fade.sync = None,
                        Beats::Match(matched) => {
                            sync.stage = Stage::Resting(Instant::now());
                            fade.incoming.pause();
                            fade.incoming.set_tempo(matched.rate);
                            return glib::ControlFlow::Continue;
                        }
                    }
                }
                begin_fade(fade);
                drop(slot);
                // No longer pending: Next and Previous are back.
                self.changed();
                return glib::ControlFlow::Continue;
            } else if fade.began.elapsed() > START_TIMEOUT {
                log::warn!("DJ: the next track did not start; giving up the fade");
                drop(slot);
                self.finish_fade();
                return glib::ControlFlow::Break;
            } else {
                return glib::ControlFlow::Continue;
            }
        }
        let Some((plan, since)) = fade.running else {
            return glib::ControlFlow::Continue;
        };
        let elapsed = since.elapsed();
        if plan.is_cut() || elapsed >= plan.fade {
            drop(slot);
            // The source is removed by returning Break, not by `finish_fade`.
            self.end_fade(false);
            return glib::ControlFlow::Break;
        }
        let gains = plan.gains(elapsed);
        fade.outgoing.set_fade_gain(Some(gains.outgoing));
        fade.incoming.set_fade_gain(Some(gains.incoming));
        let beats = fade
            .sync
            .as_mut()
            .and_then(|sync| sync.follow(&fade.outgoing, &fade.incoming));
        #[cfg(feature = "dev-hooks")]
        crate::gui::devtools::fade_sample(
            elapsed.as_secs_f64(),
            plan.fade.as_secs_f64(),
            (gains.outgoing, gains.incoming),
            beats,
            fade.incoming.tempo(),
        );
        // Twice a second is enough to follow it in the log.
        if fade.steps % 10 == 0 {
            log::info!(
                "DJ: fade {:.2}/{:.2} s  out {:.2}  in {:.2}{}",
                elapsed.as_secs_f64(),
                plan.fade.as_secs_f64(),
                gains.outgoing,
                gains.incoming,
                beats.map_or(String::new(), |(error, settled)| format!(
                    "  beats {:+.0} ms{}  tempo {:.3}",
                    error * 1000.0,
                    if settled { "" } else { " (clock settling)" },
                    fade.incoming.tempo()
                ))
            );
        }
        fade.steps += 1;
        glib::ControlFlow::Continue
    }

    /// End a running fade now: the new track at full volume, the old one
    /// paused and let go.
    fn finish_fade(&self) {
        self.end_fade(true);
    }

    fn end_fade(&self, remove_timer: bool) {
        let Some(fade) = self.0.fade.take() else {
            return;
        };
        if remove_timer {
            fade.timer.remove();
        }
        fade.incoming.set_fade_gain(None);
        start_held(&fade.incoming);
        if let Some(mut sync) = fade.sync {
            if let Some(hold) = sync.hold.take() {
                hold.remove();
            }
            match sync.stage {
                Stage::Waiting => {}
                // Cut short before the beat came: just play.
                Stage::Resting(_) | Stage::Held => {
                    fade.incoming.set_tempo(1.0);
                    fade.incoming.resume();
                }
                Stage::Matched => {
                    log::info!("DJ: beat match: {}", sync.summary());
                    fade.incoming.ease_tempo_home(TEMPO_HOME);
                }
            }
        }
        log::info!("DJ: fade done; releasing the old player");
        fade.outgoing.set_fade_gain(Some(0.0));
        release(fade.outgoing);
        self.changed();
    }

    // ---- Getting tracks ready ahead of time ----

    fn audio_only(client: &Client) -> bool {
        !client.core().dj_settings().show_video
    }

    /// The track's stream has been looked up (audio or video, as the DJ plays).
    fn is_prepared(client: &Client, video: &Video) -> bool {
        if Self::audio_only(client) {
            client.prepared_audio_playback_uri(video).is_some()
        } else {
            client.prepared_playback_uri(video).is_some()
        }
    }

    fn is_warm(&self, id: &str) -> bool {
        self.0.warm.borrow().iter().any(|w| w.id == id && w.ready)
    }

    /// Prepare the next track (`next`, once the current one is under way)
    /// and the queue row the pointer rests on, then load their players and
    /// hold them paused at the start. Called from the loop.
    fn warm_up(&self, client: &Client, next: Option<&Track>) {
        let inner = &self.0;
        let Some(window) = inner.window.upgrade() else {
            return;
        };
        let hovered = inner.hovered.borrow().clone().and_then(|id| {
            let session = inner.session.borrow();
            session
                .as_ref()?
                .upcoming()
                .iter()
                .find(|t| t.id == id)
                .cloned()
        });
        let mut wanted: Vec<Track> = next.cloned().into_iter().collect();
        if let Some(hovered) = hovered.filter(|h| wanted.iter().all(|t| t.id != h.id)) {
            wanted.push(hovered);
        }

        // What is no longer queued is let go; what finished loading says so.
        let queued: Vec<String> = inner
            .session
            .borrow()
            .as_ref()
            .map(|s| s.upcoming().iter().map(|t| t.id.clone()).collect())
            .unwrap_or_default();
        let mut warm = inner.warm.borrow_mut();
        let (keep, gone): (Vec<Warm>, Vec<Warm>) =
            warm.drain(..).partition(|w| queued.contains(&w.id));
        *warm = keep;
        gone.into_iter().for_each(|w| release(w.page));
        let to_entry = client.core().dj_settings().start_on_beat;
        for w in warm.iter_mut().filter(|w| !w.ready) {
            if !w.page.is_warm() {
                continue;
            }
            // "Start on the Beat": the loaded player goes to the track's
            // entry and waits there, so the start is as instant as at 0. Its
            // entry is being worked out meanwhile; that is waited for.
            if to_entry && w.page.entry().is_none() {
                let known = w
                    .page
                    .video()
                    .and_then(|video| client.core().beats(&video.id()))
                    .and_then(|known| known.entry);
                match known {
                    Some(entry) if entry.start > 0.0 => {
                        log::info!(
                            "DJ: warm {} goes to its entry at {:.2} s",
                            w.id,
                            entry.start
                        );
                        w.page.seek_entry(entry.start);
                        continue;
                    }
                    Some(_) => {}
                    None if w.began.elapsed() < ENTRY_WAIT => continue,
                    None => log::info!(
                        "DJ: warm {} stays at its start: no entry worked out in time",
                        w.id
                    ),
                }
            }
            if w.page.entry().is_some() && !w.page.entry_ready() {
                continue;
            }
            w.ready = true;
            log::info!(
                "DJ: warm {} ready in {:.1} s{}",
                w.id,
                w.began.elapsed().as_secs_f64(),
                w.page
                    .entry()
                    .map_or(String::new(), |at| format!(", at its entry ({at:.2} s)"))
            );
        }

        for track in &wanted {
            let video = client.dj_video(track);
            let prepared = Self::is_prepared(client, &video);
            if prepared {
                self.analyse_ahead(client, track, &video);
            }
            if warm.iter().any(|w| w.id == track.id) {
                continue;
            }
            if !prepared {
                let mut asked = inner.prepared.borrow_mut();
                if !asked.contains(&track.id) {
                    log::info!("DJ: preparing {} ({})", track.id, track.title);
                    asked.push(track.id.clone());
                    if Self::audio_only(client) {
                        client.prepare_audio_playback(&video);
                    } else {
                        client.prepare_playback(&video);
                    }
                }
                continue;
            }
            // Room for it: the least recent one nobody waits for goes.
            if warm.len() >= MAX_WARM {
                let Some(index) = warm
                    .iter()
                    .position(|w| wanted.iter().all(|t| t.id != w.id))
                else {
                    continue;
                };
                release(warm.remove(index).page);
            }
            log::info!("DJ: warming {} ({})", track.id, track.title);
            let page = window.imp().new_dj_page(&video);
            page.warm();
            warm.push(Warm {
                id: track.id.clone(),
                page,
                began: Instant::now(),
                ready: false,
            });
        }
    }

    /// With "Match Beats" or "Start on the Beat" on: work out the beats (and
    /// the entry) of a track whose stream is prepared, once, in the
    /// background. They are kept on disk, so a track that was analysed
    /// before costs nothing.
    fn analyse_ahead(&self, client: &Client, track: &Track, video: &Video) {
        let settings = client.core().dj_settings();
        if !settings.match_beats && !settings.start_on_beat {
            return;
        }
        // (Asked for again when the entry is wanted after all.)
        let entry = settings.start_on_beat;
        let key = format!("{}{}", track.id, if entry { "+entry" } else { "" });
        let mut analysed = self.0.analysed.borrow_mut();
        if analysed.contains(&key) {
            return;
        }
        let Some(uri) = client
            .prepared_audio_playback_uri(video)
            .or_else(|| client.prepared_playback_uri(video))
        else {
            return;
        };
        analysed.push(key);
        let (core, id) = (client.core(), video.id());
        let length = track.duration.map(|d| d.as_secs_f64());
        let _ = std::thread::Builder::new()
            .name("kinora-beats".into())
            .spawn(move || analyse_intro(core, id, uri, entry, length));
    }

    /// The track is about to play: its warmed page, if there is one, and how
    /// ready it is (for the log).
    fn take_ahead(&self, client: &Client, track: &Track) -> (Option<VideoPage>, &'static str) {
        let mut warm = self.0.warm.borrow_mut();
        match warm.iter().position(|w| w.id == track.id) {
            Some(index) => {
                let w = warm.remove(index);
                let readiness = if w.ready || w.page.is_warm() {
                    "warm"
                } else {
                    "warming"
                };
                (Some(w.page), readiness)
            }
            None if Self::is_prepared(client, &client.dj_video(track)) => (None, "prepared"),
            None => (None, "cold"),
        }
    }

    /// Let go of everything that was got ready (the DJ is left, or changes
    /// between audio and video).
    fn forget_ahead(&self) {
        self.0.prepared.borrow_mut().clear();
        self.0.hovered.take();
        let warm: Vec<Warm> = self.0.warm.borrow_mut().drain(..).collect();
        warm.into_iter().for_each(|w| release(w.page));
    }

    /// The panel's "Show video" was switched.
    fn set_show_video(&self, show: bool) {
        let Some(window) = self.0.window.upgrade() else {
            return;
        };
        if window.client().core().dj_settings().show_video == show {
            return;
        }
        log::info!("DJ: {}", if show { "with video" } else { "audio only" });
        crate::gui::extension_settings::store(
            &window.imp().settings,
            kinora_core::extension::builtin::dj::ID,
            kinora_core::extension::builtin::dj::SHOW_VIDEO,
            kinora_core::extension::SettingValue::Bool(show),
        );
        // What was got ready is of the other kind now.
        self.forget_ahead();
        if let Some(page) = self.0.page.borrow().as_ref() {
            page.set_audio_only(!show);
        }
    }

    // ---- The queue ----

    /// Top the queue up in the background when it runs low.
    fn refill(&self) {
        let inner = &self.0;
        if inner.refilling.get() || inner.refill_after.get().is_some_and(|t| Instant::now() < t) {
            return;
        }
        let Some(client) = self.client() else {
            return;
        };
        let request = {
            let mut session = inner.session.borrow_mut();
            let Some(session) = session.as_mut() else {
                return;
            };
            // The settings may have changed since the session began.
            session.set_options(kinora_core::dj::DjOptions {
                refill_below: session.options().refill_below,
                ..client.core().dj_settings().options()
            });
            session.refill_request()
        };
        let Some(request) = request else {
            return;
        };
        inner.refilling.set(true);
        let dj = self.clone();
        let generation = inner.generation.get();
        gspawn!(async move {
            let result = client.dj_more(request.clone()).await;
            if dj.0.generation.get() != generation {
                return;
            }
            dj.0.refilling.set(false);
            if let Some(session) = dj.0.session.borrow_mut().as_mut() {
                match result {
                    Ok(page) => {
                        let fetched = page.tracks.len();
                        let added = session.extend(&request, page);
                        log::info!(
                            "DJ: refill, {added} of {fetched} new, {} queued",
                            session.upcoming().len()
                        );
                    }
                    Err(e) => {
                        log::warn!("DJ: refill failed: {e}");
                        session.fetch_failed(&request);
                        dj.0.refill_after.set(Some(Instant::now() + REFILL_PAUSE));
                    }
                }
            }
            dj.refresh_panel();
        });
    }

    fn panel_action(&self, action: PanelAction) {
        match action {
            PanelAction::Leave => return self.leave(),
            PanelAction::ShowVideo(show) => return self.set_show_video(show),
            PanelAction::Hover(id) => {
                self.0.hovered.replace(Some(id));
                return;
            }
            PanelAction::PlayNow(id) => {
                if let Some(session) = self.0.session.borrow_mut().as_mut() {
                    session.move_to(&id, 0);
                }
                return self.skip();
            }
            PanelAction::PlayNext(id) => self.edit(|s| {
                s.move_to(&id, 0);
            }),
            PanelAction::Remove(id) => self.edit(|s| {
                s.remove(&id);
            }),
            PanelAction::Move(id, index) => self.edit(|s| {
                s.move_to(&id, index);
            }),
        }
        self.refresh_panel();
    }

    fn edit(&self, change: impl FnOnce(&mut DjSession)) {
        if let Some(session) = self.0.session.borrow_mut().as_mut() {
            change(session);
        }
    }

    /// Dev panel (see `gui/devtools`): everything the DJ holds right now.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_probe(&self) -> Option<crate::gui::devtools::DjProbe> {
        use crate::gui::devtools::{DjProbe, FadeProbe, SyncProbe, WarmProbe};
        let inner = &self.0;
        let session = inner.session.borrow();
        let session = session.as_ref()?;
        let fade = inner.fade.borrow();
        Some(DjProbe {
            seed: format!("{:?}", session.seed()),
            source: format!("{:?}", session.source()),
            upcoming: session.upcoming().iter().take(8).cloned().collect(),
            queued: session.upcoming().len(),
            played: session.history().len(),
            page: inner.page.borrow().clone(),
            fade: fade.as_ref().map(|fade| FadeProbe {
                outgoing: fade.outgoing.clone(),
                incoming: fade.incoming.clone(),
                wanted: fade.wanted.as_secs_f64(),
                since: fade.began.elapsed().as_secs_f64(),
                running: fade
                    .running
                    .map(|(plan, since)| (since.elapsed().as_secs_f64(), plan.fade.as_secs_f64())),
                readiness: fade.readiness,
                entry: match fade.entry {
                    ToEntry::No => "no",
                    ToEntry::Wanted(_) => "wanted",
                    ToEntry::Seeking(_) => "seeking",
                },
                sync: fade.sync.as_ref().map(|sync| SyncProbe {
                    stage: match sync.stage {
                        Stage::Waiting => "waiting",
                        Stage::Resting(_) => "resting",
                        Stage::Held => "held",
                        Stage::Matched => "matched",
                    },
                    incoming: sync.incoming,
                    outgoing: sync.outgoing,
                    matched: sync.matched,
                    let_go: sync.let_go.map(|at| at.elapsed().as_secs_f64()),
                    errors: sync.errors.clone(),
                    nudging: sync.nudge.is_some(),
                }),
            }),
            warm: inner
                .warm
                .borrow()
                .iter()
                .map(|w| WarmProbe {
                    id: w.id.clone(),
                    page: w.page.clone(),
                    since: w.began.elapsed().as_secs_f64(),
                    ready: w.ready,
                })
                .collect(),
            prepared: inner.prepared.borrow().clone(),
            analysed: inner.analysed.borrow().clone(),
            hovered: inner.hovered.borrow().clone(),
            starting: inner.starting.get(),
            refilling: inner.refilling.get(),
        })
    }

    /// Test hook: what the session holds, for the log.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_log(&self) {
        match self.0.session.borrow().as_ref() {
            Some(s) => log::info!(
                "DEV: DJ on, now {:?}, next {:?}, {} queued, {} played, source {:?}",
                s.current().map(|t| &t.title),
                s.upcoming()
                    .iter()
                    .take(3)
                    .map(|t| &t.title)
                    .collect::<Vec<_>>(),
                s.upcoming().len(),
                s.history().len(),
                s.source()
            ),
            None => log::info!("DEV: DJ off"),
        }
    }

    /// Test hook: switch "Show video".
    #[cfg(feature = "dev-hooks")]
    pub fn dev_toggle_video(&self) {
        if let Some(client) = self.client() {
            self.set_show_video(!client.core().dj_settings().show_video);
            self.refresh_panel();
        }
    }

    /// Test hook: switch "Match Beats".
    #[cfg(feature = "dev-hooks")]
    pub fn dev_toggle_match(&self) {
        let Some(window) = self.0.window.upgrade() else {
            return;
        };
        let on = !window.client().core().dj_settings().match_beats;
        crate::gui::extension_settings::store(
            &window.imp().settings,
            kinora_core::extension::builtin::dj::ID,
            kinora_core::extension::builtin::dj::MATCH_BEATS,
            kinora_core::extension::SettingValue::Bool(on),
        );
        log::info!("DEV: match beats {}", if on { "on" } else { "off" });
    }

    /// Test hook: switch "Start on the Beat".
    #[cfg(feature = "dev-hooks")]
    pub fn dev_toggle_entry(&self) {
        let Some(window) = self.0.window.upgrade() else {
            return;
        };
        let on = !window.client().core().dj_settings().start_on_beat;
        crate::gui::extension_settings::store(
            &window.imp().settings,
            kinora_core::extension::builtin::dj::ID,
            kinora_core::extension::builtin::dj::START_ON_BEAT,
            kinora_core::extension::SettingValue::Bool(on),
        );
        log::info!("DEV: start on the beat {}", if on { "on" } else { "off" });
    }

    /// Test hook: act on the queue as the panel's buttons would. `what` is
    /// `play-next`, `remove` or `play-now`, applied to the queue's third track.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_queue(&self, what: &str) {
        let id = self
            .0
            .session
            .borrow()
            .as_ref()
            .and_then(|s| s.upcoming().get(2).map(|t| t.id.clone()));
        let Some(id) = id else { return };
        self.panel_action(match what {
            "play-next" => PanelAction::PlayNext(id),
            "hover" => PanelAction::Hover(id),
            "remove" => PanelAction::Remove(id),
            _ => PanelAction::PlayNow(id),
        });
        self.dev_log();
    }
}

/// The seed that plays a playlist, an album or a mix (by id or link).
fn playlist_seed(id: &str, title: &str) -> Option<DjSeed> {
    DjSeed::playlist(id, Some(title.to_owned()).filter(|t| !t.is_empty()))
}

/// The video a seed takes over (it is already playing), if it is one.
fn seed_video(seed: &DjSeed) -> Option<&str> {
    match seed {
        DjSeed::Video(id) => Some(id),
        _ => None,
    }
}

/// Let a page go whose player may still run: pause only, and drop it a
/// moment later. Stopping a Clapper player, or ending one while another
/// starts, has crashed (see `VideoPage::stop`).
fn release(page: VideoPage) {
    page.stop();
    page.retire();
}

/// Start a warm track that was held back for its beat (see
/// `Dj::hold_for_beats`); nothing for any other.
fn start_held(page: &VideoPage) {
    if page.is_warm_held() {
        page.hold_warm(false);
        page.start_warm();
    }
}

/// The incoming track plays: the fade starts now, over what the old track
/// has left at most.
fn begin_fade(fade: &mut Fade) {
    let out = &fade.outgoing;
    let left = match (out.player_state(), out.duration_secs()) {
        (clapper::PlayerState::Playing, Some(d)) => {
            Duration::from_secs_f64((d - out.position_secs()).max(0.0))
        }
        _ => Duration::ZERO,
    };
    let over = fade.wanted.min(left);
    log::info!(
        "DJ: started after {:.1} s ({}){}; fading over {:.1} s",
        fade.began.elapsed().as_secs_f64(),
        fade.readiness,
        fade.incoming.entry().map_or(String::new(), |entry| format!(
            ", from its entry at {entry:.2} s (the player is at {:.2} s)",
            fade.incoming.heard_position()
        )),
        over.as_secs_f64()
    );
    fade.running = Some((fade_plan(over), Instant::now()));
}

/// A fade of `over` that starts now (a cut when there is nothing to fade).
fn fade_plan(over: Duration) -> TransitionPlan {
    if over < Duration::from_millis(100) {
        TransitionPlan::CUT
    } else {
        TransitionPlan {
            start_at: Some(Duration::ZERO),
            fade: over,
        }
    }
}

/// "3:32", or "1:02:05".
pub(crate) fn format_length(length: Option<Duration>) -> String {
    let Some(length) = length else {
        return String::new();
    };
    let s = length.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// "Album · 2019" for a song; nothing for a plain video.
fn album_line(track: &Track) -> String {
    match (&track.album, track.year) {
        (Some(album), Some(year)) => format!("{} · {year}", album.name),
        (Some(album), None) => album.name.clone(),
        (None, Some(year)) => year.to_string(),
        (None, None) => String::new(),
    }
}

// ---- The panel ------------------------------------------------------------

enum PanelAction {
    ShowVideo(bool),
    /// The pointer rests on a queue row.
    Hover(String),
    Leave,
    PlayNow(String),
    PlayNext(String),
    Remove(String),
    Move(String, usize),
}

type Handler = Rc<RefCell<Option<Box<dyn Fn(PanelAction)>>>>;

/// The DJ's parts of its page (the page itself has the record player and the
/// controls, see `video_page.blp`): the bar ("DJ on", Show Video, Leave),
/// what plays (title, artists, album), and the queue. One set of them moves
/// from page to page with the music.
struct Panel {
    bar: gtk::Box,
    now: gtk::Box,
    queue: gtk::Box,
    note: gtk::Label,
    title: gtk::Label,
    artists: gtk::Label,
    album: gtk::Label,
    count: gtk::Label,
    pending: adw::Spinner,
    show_video: gtk::ToggleButton,
    list: gtk::ListBox,
    more: gtk::Label,
    /// What is shown, to leave the widgets alone when nothing changed.
    shown_now: RefCell<Option<String>>,
    shown_queue: RefCell<Vec<String>>,
    /// The queue's artwork, small, by track id: only what the queue shows,
    /// so a row that is built again has its picture at once.
    artwork: Artwork,
    handler: Handler,
}

/// A square picture of `size` that never asks for more room than that.
fn artwork(size: i32) -> (gtk::Widget, gtk::Picture) {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    let frame = gtk::Box::builder()
        .width_request(size)
        .height_request(size)
        .build();
    let overlay = gtk::Overlay::builder()
        .child(&frame)
        .overflow(gtk::Overflow::Hidden)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["k-dj-art"])
        .build();
    overlay.add_overlay(&picture);
    (overlay.upcast(), picture)
}

/// The queue's small artwork, by track id (see [`show_artwork`]).
type Artwork = Rc<RefCell<std::collections::HashMap<String, gdk::Texture>>>;

/// The queue's artwork is 48 px: its pictures are made this many pixels
/// high (sharp at twice the size too). A thumbnail as it comes (720 px
/// wide and more) takes about 1 MB in memory; at this size, 50 kB.
const ARTWORK_PIXELS: i32 = 96;

/// Show `track`'s thumbnail in a queue row's `picture`: from `artwork` when
/// it is there, else read (through the thumbnail cache) and kept there. No
/// `Video` is made for it, so nothing else about the track is fetched.
fn show_artwork(picture: &gtk::Picture, track: &Track, artwork: &Artwork, client: &Client) {
    if let Some(texture) = artwork.borrow().get(&track.id) {
        picture.set_paintable(Some(texture));
        return;
    }
    let Some(url) = track.thumbnail.clone() else {
        return;
    };
    let (picture, id) = (picture.downgrade(), track.id.clone());
    let (artwork, client) = (artwork.clone(), client.clone());
    gspawn!(async move {
        let Some(pixbuf) = client.picture_at(url, ARTWORK_PIXELS).await else {
            return;
        };
        let texture = gdk::Texture::for_pixbuf(&pixbuf);
        if let Some(picture) = picture.upgrade() {
            picture.set_paintable(Some(&texture));
            // (Only for a row that is still shown.)
            artwork.borrow_mut().insert(id, texture);
        }
    });
}

fn label(css: &[&str], lines: i32) -> gtk::Label {
    let label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(css.to_vec())
        .build();
    if lines > 1 {
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_lines(lines);
    }
    label
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build()
}

impl Panel {
    fn new() -> Self {
        let handler: Handler = Rc::default();
        let emit = |handler: &Handler, action: PanelAction| {
            if let Some(f) = handler.borrow().as_ref() {
                f(action);
            }
        };

        // "DJ on", where the music comes from, and the way out.
        let header = gtk::Box::builder()
            .spacing(4)
            .css_classes(["k-dj-bar-row"])
            .build();
        let on = gtk::Label::builder()
            .label(gettextrs::gettext("DJ on"))
            .css_classes(["k-kicker", "k-dj-on"])
            .valign(gtk::Align::Center)
            .build();
        let note = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["k-dj-note"])
            .build();
        let leave = gtk::Button::builder()
            .label(gettextrs::gettext("Leave"))
            .tooltip_text(gettextrs::gettext(
                "Leave the DJ: this video plays on, with Up next as usual",
            ))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "k-dj-leave"])
            .build();
        leave.connect_clicked(glib::clone!(
            #[strong]
            handler,
            move |_| emit(&handler, PanelAction::Leave)
        ));
        // Audio first; the video is one switch away.
        let show_video = gtk::ToggleButton::builder()
            .icon_name("camera-video-symbolic")
            .tooltip_text(gettextrs::gettext(
                "Show Video (off: audio only, tracks start faster)",
            ))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular", "k-dj-video"])
            .build();
        show_video.connect_toggled(glib::clone!(
            #[strong]
            handler,
            move |button| emit(&handler, PanelAction::ShowVideo(button.is_active()))
        ));
        // The record player fills the screen too (the page's own action).
        let fullscreen = icon_button(
            "view-fullscreen-symbolic",
            &gettextrs::gettext("Fullscreen (F)"),
        );
        fullscreen.set_action_name(Some("video.fullscreen"));

        // What plays: large, under the record.
        let now = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .css_classes(["k-dj-now"])
            .build();
        let centred = |css: &[&str], lines: i32| {
            let label = label(css, lines);
            label.set_xalign(0.5);
            label.set_justify(gtk::Justification::Center);
            label
        };
        let title = centred(&["k-dj-title"], 2);
        title.set_accessible_role(gtk::AccessibleRole::Heading);
        let artists = centred(&["k-dj-artists"], 1);
        let album = centred(&["k-dj-album"], 1);
        now.append(&title);
        now.append(&artists);
        now.append(&album);
        // Shown while the next track is asked for and has not started.
        let pending = adw::Spinner::builder()
            .width_request(16)
            .height_request(16)
            .valign(gtk::Align::Center)
            .visible(false)
            .tooltip_text(gettextrs::gettext("The next track is starting"))
            .build();
        header.append(&on);
        header.append(&note);
        header.append(&pending);
        header.append(&show_video);
        header.append(&fullscreen);
        header.append(&leave);

        // The queue.
        let queue_header = gtk::Box::builder().spacing(8).build();
        let heading = gtk::Label::builder()
            .label(gettextrs::gettext("Up next"))
            .xalign(0.0)
            .hexpand(true)
            .accessible_role(gtk::AccessibleRole::Heading)
            .css_classes(["k-row-title"])
            .build();
        let count = gtk::Label::builder()
            .css_classes(["dim-label", "numeric"])
            .build();
        queue_header.append(&heading);
        queue_header.append(&count);
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(true)
            .css_classes(["k-upnext-list", "k-dj-queue"])
            .valign(gtk::Align::Start)
            .build();
        list.connect_row_activated(glib::clone!(
            #[strong]
            handler,
            move |_, row| emit(&handler, PanelAction::PlayNow(row.widget_name().into()))
        ));
        let more = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["k-dj-note"])
            .margin_start(8)
            .build();
        let queue = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .css_classes(["k-dj-queue-box"])
            .build();
        queue.append(&queue_header);
        queue.append(&list);
        queue.append(&more);

        Self {
            bar: header,
            now,
            queue,
            note,
            title,
            artists,
            album,
            count,
            pending,
            show_video,
            list,
            more,
            shown_now: RefCell::default(),
            shown_queue: RefCell::default(),
            artwork: Artwork::default(),
            handler,
        }
    }

    /// The bar, what plays, and the queue: for the page's three places.
    fn parts(&self) -> [&gtk::Widget; 3] {
        [
            self.bar.upcast_ref(),
            self.now.upcast_ref(),
            self.queue.upcast_ref(),
        ]
    }

    fn set_pending(&self, pending: bool) {
        self.pending.set_visible(pending);
    }

    fn connect(&self, f: impl Fn(PanelAction) + 'static) {
        self.handler.replace(Some(Box::new(f)));
    }

    /// Take the parts out of their page and forget what they showed.
    fn clear(&self) {
        for part in self.parts() {
            if let Some(parent) = part.parent().and_downcast::<gtk::Box>() {
                parent.remove(part);
            }
        }
        self.shown_now.take();
        self.shown_queue.take();
        self.artwork.borrow_mut().clear();
        self.list.remove_all();
    }

    fn show(&self, session: &DjSession, client: &Client) {
        let show_video = client.core().dj_settings().show_video;
        if self.show_video.is_active() != show_video {
            self.show_video.set_active(show_video);
        }
        self.note
            .set_label(&match (session.source(), session.personal()) {
                (RadioSource::YouTube, _) => gettextrs::gettext("From YouTube"),
                (RadioSource::Music, true) => gettextrs::gettext("Picked for you"),
                (RadioSource::Music, false) => String::new(),
            });

        if let Some(track) = session.current()
            && self.shown_now.borrow().as_deref() != Some(track.id.as_str())
        {
            self.shown_now.replace(Some(track.id.clone()));
            self.title.set_label(&track.title);
            self.artists.set_label(&track.artists_line());
            let album = album_line(track);
            self.album.set_label(&album);
            self.album.set_visible(!album.is_empty());
        }

        let upcoming = session.upcoming();
        self.count.set_label(&upcoming.len().to_string());
        let hidden = upcoming.len().saturating_sub(SHOWN);
        self.more.set_visible(hidden > 0);
        if hidden > 0 {
            self.more
                .set_label(&gettextrs::gettext("and {} more").replace("{}", &hidden.to_string()));
        }
        let ids: Vec<String> = upcoming.iter().take(SHOWN).map(|t| t.id.clone()).collect();
        if *self.shown_queue.borrow() == ids {
            return;
        }
        self.list.remove_all();
        self.artwork.borrow_mut().retain(|id, _| ids.contains(id));
        for (index, track) in upcoming.iter().take(SHOWN).enumerate() {
            self.list.append(&self.row(track, index, client));
        }
        self.shown_queue.replace(ids);
    }

    /// One queued track: artwork, title, artists, length, and on hover or
    /// focus "play next" and "remove". Rows can be dragged to reorder.
    fn row(&self, track: &Track, index: usize, client: &Client) -> gtk::ListBoxRow {
        let handler = &self.handler;
        let emit = |handler: &Handler, action: PanelAction| {
            if let Some(f) = handler.borrow().as_ref() {
                f(action);
            }
        };
        let line = gtk::Box::builder().spacing(10).build();
        let (art_frame, art) = artwork(48);
        show_artwork(&art, track, &self.artwork, client);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(1)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        let title = label(&["k-dj-row-title"], 1);
        title.set_label(&track.title);
        let artists = label(&["k-card-meta"], 1);
        artists.set_label(&track.artists_line());
        text.append(&title);
        text.append(&artists);
        let length = gtk::Label::builder()
            .label(format_length(track.duration))
            .valign(gtk::Align::Center)
            .css_classes(["k-card-meta", "numeric", "k-dj-length"])
            .build();
        let actions = gtk::Box::builder()
            .css_classes(["k-dj-row-actions"])
            .build();
        let play_next = icon_button("go-top-symbolic", &gettextrs::gettext("Play Next"));
        // The first one is next already; its button keeps its place, unseen.
        if index == 0 {
            play_next.set_opacity(0.0);
            play_next.set_sensitive(false);
        }
        let remove = icon_button(
            "window-close-symbolic",
            &gettextrs::gettext("Remove from Queue"),
        );
        let id = track.id.clone();
        play_next.connect_clicked(glib::clone!(
            #[strong]
            handler,
            #[strong]
            id,
            move |_| emit(&handler, PanelAction::PlayNext(id.clone()))
        ));
        remove.connect_clicked(glib::clone!(
            #[strong]
            handler,
            #[strong]
            id,
            move |_| emit(&handler, PanelAction::Remove(id.clone()))
        ));
        actions.append(&play_next);
        actions.append(&remove);
        // The length stands where the two buttons appear.
        length.set_halign(gtk::Align::End);
        length.set_can_target(false);
        let end = gtk::Overlay::builder().child(&actions).build();
        end.add_overlay(&length);
        line.append(&art_frame);
        line.append(&text);
        line.append(&end);

        let row = gtk::ListBoxRow::builder()
            .child(&line)
            .name(track.id.as_str())
            .tooltip_text(gettextrs::gettext("Play now, or drag to reorder"))
            .build();
        row.update_property(&[gtk::accessible::Property::Label(&format!(
            "{}, {}",
            track.title,
            track.artists_line()
        ))]);

        // The pointer resting on a row gets its track ready, so that picking
        // it mixes in at once.
        let motion = gtk::EventControllerMotion::new();
        let resting: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
        motion.connect_enter(glib::clone!(
            #[strong]
            handler,
            #[strong]
            id,
            #[strong]
            resting,
            move |_, _, _| {
                let (handler, id, slot) = (handler.clone(), id.clone(), resting.clone());
                let source = glib::timeout_add_local_once(HOVER_DELAY, move || {
                    slot.take();
                    emit(&handler, PanelAction::Hover(id));
                });
                if let Some(old) = resting.replace(Some(source)) {
                    old.remove();
                }
            }
        ));
        motion.connect_leave(move |_| {
            if let Some(source) = resting.take() {
                source.remove();
            }
        });
        row.add_controller(motion);

        // Reorder by dragging a row onto another: it takes that one's place.
        let drag = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        drag.connect_prepare(glib::clone!(
            #[strong]
            id,
            move |_, _, _| Some(gdk::ContentProvider::for_value(&id.to_value()))
        ));
        drag.connect_drag_begin(|source, _| {
            if let Some(row) = source.widget() {
                source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&row))), 24, 24);
            }
        });
        row.add_controller(drag);
        let drop = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        drop.connect_drop(glib::clone!(
            #[strong]
            handler,
            #[strong]
            id,
            move |_, value, _, _| match value.get::<String>() {
                Ok(dragged) if dragged != id => {
                    emit(&handler, PanelAction::Move(dragged, index));
                    true
                }
                _ => false,
            }
        ));
        row.add_controller(drop);
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_like_a_clock() {
        assert_eq!(format_length(Some(Duration::from_secs(212))), "3:32");
        assert_eq!(format_length(Some(Duration::from_secs(3725))), "1:02:05");
        assert_eq!(format_length(None), "");
    }

    #[test]
    fn nothing_to_fade_is_a_cut() {
        assert!(fade_plan(Duration::ZERO).is_cut());
        let plan = fade_plan(Duration::from_secs(6));
        assert!(!plan.is_cut());
        // Equal power: both at about 0.71 halfway.
        let half = plan.gains(Duration::from_secs(3));
        assert!((half.outgoing - half.incoming).abs() < 1e-9);
        assert!((half.outgoing - kinora_core::dj::equal_power(0.5).outgoing).abs() < 1e-9);
    }
}
