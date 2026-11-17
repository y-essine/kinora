// SPDX-License-Identifier: GPL-3.0-or-later

//! Kinora: the Dev panel (feature `dev-hooks` only; release builds have none
//! of this). A window beside the app that shows what the player and the DJ
//! are doing, live: both decks' waveforms with their beats, what is buffered,
//! the crossfade and its beat matching, the queue's analysis, the process,
//! and the log. The same data is written as JSON by `dev-dump`, for a test
//! session that cannot look (see CONTRIBUTING-KINORA.md, "Dev panel").
//!
//! - `logs`: the log's tail, around the app's logger.
//! - `peaks`: waveforms, reduced once and then only drawn.
//! - `state`: one [`state::Snapshot`] of everything, for the panel and the dump.
//! - `draw`, `panel`: the window.
//!
//! The rest of the app knows this module by a handful of calls: the probes
//! below (filled in by `dj.rs` and `video_page.rs`), [`fade_sample`],
//! [`page_born`], and [`attach`] / [`toggle`] / [`dump`] from the window.
//! While the panel is closed nothing here runs but a heartbeat four times a
//! second (the main loop's lag, for the next time it is opened or dumped).

mod draw;
mod logs;
mod panel;
mod peaks;
pub mod state;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use kinora_core::beats::{BeatGrid, BeatMatch};
use kinora_core::dj::Track;

use crate::gui::video_page::VideoPage;
use crate::gui::window::Window;

pub use logs::install as install_logger;

/// What only a video page knows of its player (see `VideoPage::dev_probe`).
pub struct DeckProbe {
    pub warming: bool,
    pub warm_held: bool,
    pub audio_stream: bool,
    pub fast_start: bool,
    pub fade_base: Option<f64>,
    pub scratching: bool,
    /// The sample rate and channels that go through the page's tap.
    pub format: (u32, u32),
    pub engaged: bool,
    pub pipeline: Option<gst::Element>,
    /// Seconds since the page was made.
    pub age: f64,
}

/// What the DJ holds (see `Dj::dev_probe`).
pub struct DjProbe {
    pub seed: String,
    pub source: String,
    /// The queue's first tracks.
    pub upcoming: Vec<Track>,
    pub queued: usize,
    pub played: usize,
    pub page: Option<VideoPage>,
    pub fade: Option<FadeProbe>,
    pub warm: Vec<WarmProbe>,
    pub prepared: Vec<String>,
    pub analysed: Vec<String>,
    pub hovered: Option<String>,
    pub starting: bool,
    pub refilling: bool,
}

pub struct FadeProbe {
    pub outgoing: VideoPage,
    pub incoming: VideoPage,
    pub wanted: f64,
    /// Seconds since the fade was asked for.
    pub since: f64,
    /// Once both play: seconds into the fade, and its length.
    pub running: Option<(f64, f64)>,
    pub readiness: &'static str,
    pub entry: &'static str,
    pub sync: Option<SyncProbe>,
}

pub struct SyncProbe {
    pub stage: &'static str,
    pub incoming: BeatGrid,
    pub outgoing: Option<BeatGrid>,
    pub matched: Option<BeatMatch>,
    pub let_go: Option<f64>,
    pub errors: Vec<f64>,
    pub nudging: bool,
}

pub struct WarmProbe {
    pub id: String,
    pub page: VideoPage,
    pub since: f64,
    pub ready: bool,
}

/// One step of a fade, as the DJ took it.
#[derive(Clone, Copy, serde::Serialize)]
pub struct FadeSample {
    /// Seconds into the fade.
    pub t: f32,
    pub out: f32,
    #[serde(rename = "in")]
    pub inc: f32,
    /// How far the beats are apart, in milliseconds (a matched fade).
    pub beat_ms: Option<f32>,
    /// The reading counts (the incoming player's clock has settled).
    pub settled: bool,
    /// The incoming track's tempo.
    pub tempo: f32,
}

/// The fade that runs, or the last one.
#[derive(Default)]
pub struct FadeLog {
    pub total: f64,
    pub samples: Vec<FadeSample>,
}

/// The main loop's lag: how late a timer came, lately.
pub struct Heart {
    /// (seconds since the app started, milliseconds late).
    pub lags: VecDeque<(f64, f32)>,
    pub stalls: u32,
}

/// A timer that came this late was a stall, in milliseconds.
pub const STALL_MS: f32 = 100.0;
/// How much of the lag is kept, in seconds.
pub const HEART_KEPT: f64 = 60.0;

impl Heart {
    pub fn beat(&mut self, late: Duration) {
        let (now, late) = (logs::uptime(), late.as_secs_f32() * 1000.0);
        if late >= STALL_MS {
            self.stalls += 1;
        }
        while self
            .lags
            .front()
            .is_some_and(|(at, _)| *at < now - HEART_KEPT)
        {
            self.lags.pop_front();
        }
        self.lags.push_back((now, late));
    }
}

thread_local! {
    static PAGES: RefCell<Vec<glib::WeakRef<VideoPage>>> = const { RefCell::new(Vec::new()) };
    static FADE: RefCell<FadeLog> = RefCell::new(FadeLog::default());
    static HEART: RefCell<Heart> = RefCell::new(Heart { lags: VecDeque::new(), stalls: 0 });
    static COST: std::cell::Cell<state::PanelCost> = const { std::cell::Cell::new(state::PanelCost { collect_ms: 0.0, show_ms: 0.0, draw_ms: 0.0 }) };
    static PANEL: RefCell<Option<panel::Panel>> = const { RefCell::new(None) };
    static COLLECTOR: RefCell<state::Collector> = RefCell::new(state::Collector::default());
}

/// A video page was made (its player lives as long as it does).
pub fn page_born(page: &VideoPage) {
    PAGES.with_borrow_mut(|pages| {
        pages.retain(|page| page.upgrade().is_some());
        pages.push(page.downgrade());
    });
}

/// The video pages that are alive, oldest first.
fn live_pages() -> Vec<VideoPage> {
    PAGES.with_borrow(|pages| pages.iter().filter_map(|page| page.upgrade()).collect())
}

/// A step of the DJ's fade (called from its loop, 20 times a second while
/// one runs): kept for the panel's curves and the dump.
pub fn fade_sample(
    elapsed: f64,
    total: f64,
    gains: (f64, f64),
    beats: Option<(f64, bool)>,
    tempo: f64,
) {
    FADE.with_borrow_mut(|fade| {
        if fade
            .samples
            .last()
            .is_some_and(|last| elapsed < last.t as f64)
        {
            fade.samples.clear();
        }
        fade.total = total;
        if fade.samples.len() < 4000 {
            fade.samples.push(FadeSample {
                t: elapsed as f32,
                out: gains.0 as f32,
                inc: gains.1 as f32,
                beat_ms: beats.map(|(error, _)| (error * 1000.0) as f32),
                settled: beats.is_some_and(|(_, settled)| settled),
                tempo: tempo as f32,
            });
        }
    });
}

/// Set the panel up for `window`: its shortcut (Ctrl+Shift+D, F12), the
/// heartbeat, and `KH_DUMP_EVERY=SECONDS`. The panel itself is only built
/// when it is first shown.
pub fn attach(window: &Window) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        window,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, state| {
            if is_shortcut(key, state) {
                toggle(&window);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    ));
    window.add_controller(keys);

    const BEAT: Duration = Duration::from_millis(250);
    let mut last = Instant::now();
    glib::timeout_add_local(BEAT, move || {
        let now = Instant::now();
        HEART.with_borrow_mut(|heart| heart.beat((now - last).saturating_sub(BEAT)));
        last = now;
        glib::ControlFlow::Continue
    });

    if let Some(every) = std::env::var("KH_DUMP_EVERY")
        .ok()
        .and_then(|every| every.parse::<f64>().ok())
        .filter(|every| *every >= 0.1)
    {
        let window = window.downgrade();
        glib::timeout_add_local(Duration::from_secs_f64(every), move || {
            let Some(window) = window.upgrade() else {
                return glib::ControlFlow::Break;
            };
            write_dump(&window, false);
            glib::ControlFlow::Continue
        });
    }

    window.connect_close_request(|_| {
        if let Some(panel) = PANEL.take() {
            panel.close();
        }
        glib::Propagation::Proceed
    });
}

/// Ctrl+Shift+D (taken from GTK's inspector, which stays on Ctrl+Shift+I) or F12.
pub(crate) fn is_shortcut(key: gtk::gdk::Key, state: gtk::gdk::ModifierType) -> bool {
    use gtk::gdk::{Key, ModifierType};
    key == Key::F12
        || (matches!(key, Key::D | Key::d)
            && state.contains(ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK))
}

/// Show the panel, or hide it.
pub fn toggle(window: &Window) {
    let existing = PANEL.with_borrow(|panel| panel.as_ref().map(|panel| panel.window()));
    let panel = existing.unwrap_or_else(|| {
        let panel = panel::Panel::new(window);
        let shown = panel.window();
        PANEL.set(Some(panel));
        shown
    });
    if panel.is_visible() {
        panel.set_visible(false);
        log::info!("DEV: panel closed");
    } else {
        panel.present();
        log::info!("DEV: panel open");
    }
}

/// Everything the panel knows, now.
fn collect(window: &Window, what: state::What) -> state::Snapshot {
    COLLECTOR.with_borrow_mut(|collector| collector.collect(window, what))
}

/// Where `dev-dump` writes: `KH_DUMP`, or `.dev/state.json`.
pub fn dump_path() -> std::path::PathBuf {
    std::env::var_os("KH_DUMP")
        .map(Into::into)
        .unwrap_or_else(|| ".dev/state.json".into())
}

/// Write the JSON snapshot (see CONTRIBUTING-KINORA.md for what is in it).
pub fn dump(window: &Window) {
    write_dump(window, true);
}

fn write_dump(window: &Window, say: bool) {
    let path = dump_path();
    let snapshot = collect(window, state::What::All);
    let written = serde_json::to_vec_pretty(&snapshot)
        .map_err(|e| e.to_string())
        .and_then(|json| {
            if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            // Whole or not at all: a reader never sees half a file.
            let part = path.with_extension("json.part");
            std::fs::write(&part, json).map_err(|e| e.to_string())?;
            std::fs::rename(&part, &path).map_err(|e| e.to_string())
        });
    match written {
        Ok(()) if say => log::info!("DEV: state written to {}", path.display()),
        Ok(()) => {}
        Err(e) => log::warn!("DEV: could not write {}: {e}", path.display()),
    }
}
