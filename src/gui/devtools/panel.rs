// SPDX-License-Identifier: GPL-3.0-or-later

//! The Dev panel's window: four pages (Decks, Queue, System, Log) over one
//! [`Snapshot`], taken [`TICK`] apart while the window is on screen. Hidden,
//! it has no timer and takes no snapshots.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib;

use super::state::{BeatsInfo, Deck, Fade, Snapshot, What};
use super::{draw, logs, peaks};
use crate::gui::window::Window;

/// How often the waveforms move: 20 times a second.
const TICK: Duration = Duration::from_millis(50);
/// A snapshot is taken (and the numbers change) every this many ticks: 5
/// times a second.
const SNAPSHOT_EVERY: u32 = 4;
/// The queue's rows.
const ROWS: usize = 8;
/// The log view holds at most this many lines.
const LOG_LINES: i32 = 3000;

pub struct Panel {
    inner: Rc<Inner>,
}

struct Tile {
    value: gtk::Label,
    note: gtk::Label,
    /// The value's colour, as it was last set.
    kind: Cell<&'static str>,
}

struct DeckCard {
    letter: gtk::Label,
    role: gtk::Label,
    title: gtk::Label,
    sub: gtk::Label,
    status: gtk::Label,
    wave: gtk::DrawingArea,
    overview: gtk::DrawingArea,
    /// Time, left, buffer, gain, tempo; BPM, entry, stream, format, ready.
    tiles: [Tile; 10],
    /// What was drawn last (see [`drawn`]): nothing new, nothing redrawn.
    drawn: Cell<u64>,
}

/// A number that changes when what a deck's waveforms show does.
fn drawn(deck: Option<&Deck>) -> u64 {
    use std::hash::{Hash, Hasher};
    let Some(deck) = deck else { return 0 };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    deck.id.hash(&mut hasher);
    deck.role.hash(&mut hasher);
    for number in [
        deck.position,
        deck.duration.unwrap_or(0.0),
        deck.buffered_ahead.unwrap_or(0.0),
        deck.decoded_to.unwrap_or(0.0),
        deck.waveform.known_seconds,
        deck.grid
            .as_ref()
            .map_or(0.0, |grid| grid.bpm + grid.anchor),
        deck.beats.entry.as_ref().map_or(0.0, |entry| entry.start),
    ] {
        number.to_bits().hash(&mut hasher);
    }
    hasher.finish() | 1
}

struct Mix {
    status: gtk::Label,
    curves: gtk::DrawingArea,
    offsets: gtk::DrawingArea,
    /// Progress, gains, stretch, offset, tempos, stage.
    tiles: [Tile; 6],
    notes: [gtk::Label; 2],
    /// What the graphs showed last (the fade's phase and how far it is).
    drawn: Cell<(u8, u64)>,
}

struct Queue {
    video: [gtk::Label; 8],
    rows: Vec<[gtk::Label; 7]>,
    warm: gtk::Label,
}

struct System {
    /// RSS, CPU, threads, files, players; uptime, lag, worst, stalls, log.
    tiles: [Tile; 10],
    lag: gtk::DrawingArea,
    players: gtk::Label,
    threads: gtk::Label,
    app: gtk::Label,
}

struct Log {
    view: gtk::TextView,
    level: gtk::DropDown,
    search: gtk::SearchEntry,
    follow: gtk::ToggleButton,
    count: gtk::Label,
    /// The newest line that was looked at.
    seen: Cell<u64>,
}

struct Inner {
    main: glib::WeakRef<Window>,
    window: adw::Window,
    stack: adw::ViewStack,
    snapshot: RefCell<Snapshot>,
    timer: RefCell<Option<glib::SourceId>>,
    last: Cell<Instant>,
    /// Counts the looks, and the page the last one was for.
    looks: Cell<u32>,
    looked_at: RefCell<String>,
    /// How long the drawing took since the last look.
    drawn: Cell<Duration>,
    chips: [gtk::Label; 6],
    decks: [DeckCard; 2],
    mix: Mix,
    queue: Queue,
    system: System,
    log: Log,
}

fn label(css: &[&str], text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_css_classes(css);
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

fn chip(text: &str) -> gtk::Label {
    let chip = gtk::Label::new(Some(text));
    chip.set_css_classes(&["k-dev-chip"]);
    chip.set_valign(gtk::Align::Center);
    chip
}

fn set_chip(chip: &gtk::Label, text: &str, kind: &str) {
    chip.set_text(text);
    if !chip.has_css_class(kind) {
        chip.set_css_classes(&["k-dev-chip", kind]);
    }
}

fn tile(caption: &str) -> (gtk::Box, Tile) {
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 1);
    frame.add_css_class("k-dev-tile");
    frame.set_hexpand(true);
    let value = label(&["k-dev-val"], "–");
    let note = label(&["k-dev-note"], "");
    // (The tiles share the row evenly, whatever they say.)
    value.set_width_chars(6);
    frame.append(&label(&["k-dev-cap"], caption));
    frame.append(&value);
    frame.append(&note);
    (
        frame,
        Tile {
            value,
            note,
            kind: Cell::new(""),
        },
    )
}

impl Tile {
    fn set(&self, value: &str, note: &str, kind: &'static str) {
        self.value.set_text(value);
        self.note.set_text(note);
        if self.kind.replace(kind) != kind {
            if kind.is_empty() {
                self.value.set_css_classes(&["k-dev-val"]);
            } else {
                self.value.set_css_classes(&["k-dev-val", kind]);
            }
        }
    }
}

/// A row of tiles that share the width evenly.
fn tiles<const N: usize>(captions: [&str; N], per_row: usize) -> (gtk::Grid, [Tile; N]) {
    let grid = gtk::Grid::new();
    grid.set_column_homogeneous(true);
    grid.set_column_spacing(6);
    grid.set_row_spacing(6);
    let mut index = 0;
    let made = captions.map(|caption| {
        let (frame, made) = tile(caption);
        grid.attach(
            &frame,
            (index % per_row) as i32,
            (index / per_row) as i32,
            1,
            1,
        );
        index += 1;
        made
    });
    (grid, made)
}

fn canvas(height: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_height(height);
    area.set_hexpand(true);
    area.add_css_class("k-dev-canvas");
    area.set_overflow(gtk::Overflow::Hidden);
    area
}

fn card() -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("k-dev-card");
    card
}

fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    format!("{}:{:04.1}", (seconds / 60.0) as u32, seconds % 60.0)
}

fn short_clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    format!("{}:{:02}", (seconds / 60.0) as u32, (seconds % 60.0) as u32)
}

/// 166910487 as "166 910 487".
fn grouped(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(digit);
    }
    out
}

fn status_kind(status: &str) -> &'static str {
    match status {
        "playing" => "mint",
        "warm" | "held" | "prepared" => "cyan",
        "warming" | "buffering" | "preparing" | "scratching" => "amber",
        "stopped" | "failed" => "red",
        _ => "dim",
    }
}

fn beats_kind(status: &str) -> &'static str {
    match status {
        "cached" => "mint",
        "analysing" | "pending" => "amber",
        "failed" => "red",
        _ => "dim",
    }
}

fn beats_text(beats: &BeatsInfo) -> String {
    match (beats.bpm, beats.confidence) {
        (Some(bpm), Some(confidence)) => format!(
            "{bpm:.1} BPM  c {confidence:.2}{}",
            if beats.usable { "" } else { " (vague)" }
        ),
        _ => beats.error.clone().unwrap_or_default(),
    }
}

impl DeckCard {
    fn new(letter: &str) -> (gtk::Box, Self) {
        let root = card();
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let badge = gtk::Label::new(Some(letter));
        badge.set_css_classes(&["k-dev-letter", "idle"]);
        badge.set_valign(gtk::Align::Center);
        let names = gtk::Box::new(gtk::Orientation::Vertical, 1);
        names.set_hexpand(true);
        let title = label(&["k-dev-title"], "Empty deck");
        let sub = label(&["k-dev-sub"], "");
        names.append(&title);
        names.append(&sub);
        let (role, status) = (chip("–"), chip("–"));
        head.append(&badge);
        head.append(&names);
        head.append(&role);
        head.append(&status);
        let (wave, overview) = (canvas(96), canvas(20));
        let (grid, tiles) = tiles(
            [
                "TIME", "LEFT", "BUFFERED", "GAIN", "TEMPO", "BEATS", "ENTRY", "STREAM", "FORMAT",
                "WARM",
            ],
            5,
        );
        root.append(&head);
        root.append(&wave);
        root.append(&overview);
        root.append(&grid);
        (
            root,
            Self {
                letter: badge,
                role,
                title,
                sub,
                status,
                wave,
                overview,
                tiles,
                drawn: Cell::new(u64::MAX),
            },
        )
    }

    fn show(&self, deck: Option<&Deck>) {
        if self.drawn.replace(drawn(deck)) != drawn(deck) {
            self.wave.queue_draw();
            self.overview.queue_draw();
        }
        let Some(deck) = deck else {
            self.title.set_text("Empty deck");
            self.sub.set_text("");
            self.letter.set_css_classes(&["k-dev-letter", "idle"]);
            set_chip(&self.role, "–", "dim");
            set_chip(&self.status, "–", "dim");
            self.tiles.iter().for_each(|tile| tile.set("–", "", "dim"));
            return;
        };
        let audible = matches!(deck.role.as_str(), "live" | "out" | "in" | "video");
        self.letter.set_css_classes(if audible {
            &["k-dev-letter"]
        } else {
            &["k-dev-letter", "idle"]
        });
        self.title.set_text(&deck.title);
        self.sub.set_text(&format!(
            "{} · {} · waveform: {} ({:.0} s)",
            deck.id, deck.artist, deck.waveform.source, deck.waveform.known_seconds
        ));
        let (role, kind) = match deck.role.as_str() {
            "live" => ("LIVE", "amber"),
            "out" => ("FADING OUT", "amber"),
            "in" => ("FADING IN", "cyan"),
            "next" => ("NEXT · LOADED", "cyan"),
            "queued" => ("NEXT · NO PLAYER", "dim"),
            _ => ("VIDEO", "dim"),
        };
        set_chip(&self.role, role, kind);
        set_chip(
            &self.status,
            &deck.status.to_uppercase(),
            status_kind(&deck.status),
        );

        let [
            time,
            left,
            buffered,
            gain,
            tempo,
            beats,
            entry,
            stream,
            format,
            ready,
        ] = &self.tiles;
        match deck.duration {
            Some(duration) => {
                time.set(
                    &clock(deck.position),
                    &format!("of {}", clock(duration)),
                    "",
                );
                let remains = duration - deck.position;
                left.set(
                    &format!("-{}", clock(remains)),
                    &format!("{:.0} %", deck.position / duration * 100.0),
                    if remains < 20.0 && audible {
                        "amber"
                    } else {
                        ""
                    },
                );
            }
            None => {
                time.set(&clock(deck.position), "length unknown", "");
                left.set("–", "", "dim");
            }
        }
        match deck.buffered_ahead {
            Some(ahead) => buffered.set(
                &format!("+{ahead:.1} s"),
                &deck
                    .decoded_to
                    .map_or(String::new(), |flow| format!("decoded to {}", clock(flow))),
                if ahead < 3.0 { "red" } else { "mint" },
            ),
            None => buffered.set(
                "–",
                if deck.loaded {
                    "not known"
                } else {
                    "not loaded"
                },
                "dim",
            ),
        }
        gain.set(
            &format!("{:.2}", deck.gain),
            &format!("volume {:.2}", deck.volume),
            if deck.base_volume.is_some() {
                "amber"
            } else {
                ""
            },
        );
        let stretch = (deck.tempo - 1.0) * 100.0;
        tempo.set(
            &format!("{stretch:+.2} %"),
            if deck.can_stretch {
                "stretch ready"
            } else {
                "fixed"
            },
            if stretch.abs() > 0.01 { "amber" } else { "" },
        );
        match (&deck.grid, deck.beats.bpm) {
            (Some(grid), _) => beats.set(
                &format!("{:.1}", grid.bpm * deck.tempo),
                &format!("{} · conf {:.2}", grid.from, grid.confidence),
                if grid.confidence >= 0.12 { "" } else { "dim" },
            ),
            (None, Some(bpm)) => beats.set(&format!("{bpm:.1}"), &deck.beats.status, ""),
            (None, None) => beats.set(
                "–",
                deck.beats.error.as_deref().unwrap_or(&deck.beats.status),
                beats_kind(&deck.beats.status),
            ),
        }
        match deck.beats.entry.as_ref() {
            Some(found) if found.start > 0.0 => entry.set(
                &format!("{:.2} s", found.start),
                &match deck.entry_target {
                    Some(_) if deck.entry_ready => "player is there".to_owned(),
                    Some(_) => "player on its way".to_owned(),
                    None => found.found.clone(),
                },
                "cyan",
            ),
            Some(found) => entry.set(
                "start",
                match found.found.as_str() {
                    "it gets going from its start" => "goes from its start",
                    "no clear beat that comes in and stays" => "no clear beat",
                    "nothing to go by" => "nothing to go by",
                    _ => "the beat is too far in",
                },
                "",
            ),
            None => entry.set("–", "not looked for", "dim"),
        }
        let prepared = deck.stream.kind.starts_with("prepared");
        stream.set(
            deck.stream
                .kind
                .trim_start_matches("prepared ")
                .trim_end_matches(" fallback"),
            &match (deck.stream.file_bytes, deck.stream.uri.as_deref()) {
                (Some(bytes), _) if prepared => {
                    format!("prepared, {:.1} kB", bytes as f64 / 1000.0)
                }
                (Some(bytes), _) => format!("{:.1} MB", bytes as f64 / 1e6),
                (None, Some(uri)) if uri.starts_with("file:") => "the file is gone".to_owned(),
                (None, Some(_)) if deck.stream.kind.starts_with("yt-dlp") => {
                    "the fallback".to_owned()
                }
                (None, Some(uri)) => uri
                    .split("://")
                    .nth(1)
                    .unwrap_or(uri)
                    .chars()
                    .take(22)
                    .collect(),
                (None, None) => String::new(),
            },
            if deck.stream.kind.starts_with("yt-dlp") {
                "amber"
            } else {
                ""
            },
        );
        if deck.stream.sample_rate > 0 {
            format.set(
                &format!(
                    "{:.1} kHz · {} ch",
                    deck.stream.sample_rate as f64 / 1000.0,
                    deck.stream.channels
                ),
                &format!(
                    "{}{}",
                    deck.stream.codec.as_deref().unwrap_or("codec not known"),
                    if deck.stream.bitrate > 0 {
                        format!(" · {} kb/s", deck.stream.bitrate / 1000)
                    } else {
                        String::new()
                    }
                ),
                "",
            );
        } else {
            format.set("–", "", "dim");
        }
        match deck.warm.as_ref() {
            Some(warm) => ready.set(
                if warm.ready { "ready" } else { "warming" },
                &format!("for {:.1} s", warm.since),
                if warm.ready { "cyan" } else { "amber" },
            ),
            None => ready.set("–", "not held ahead", "dim"),
        }
    }
}

impl Mix {
    fn new() -> (gtk::Box, Self) {
        let root = card();
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let heading = label(&["k-dev-h"], "CROSSFADE");
        heading.set_hexpand(true);
        let status = chip("IDLE");
        head.append(&heading);
        head.append(&status);
        let graphs = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        graphs.set_homogeneous(true);
        let (curves, offsets) = (canvas(84), canvas(84));
        graphs.append(&curves);
        graphs.append(&offsets);
        let (grid, tiles) = tiles(
            [
                "PROGRESS",
                "OUT / IN",
                "STRETCH",
                "BEATS APART",
                "TEMPOS",
                "MATCH",
            ],
            6,
        );
        let notes = [label(&["k-dev-note"], ""), label(&["k-dev-note"], "")];
        root.append(&head);
        root.append(&graphs);
        root.append(&grid);
        notes.iter().for_each(|note| root.append(note));
        (
            root,
            Self {
                status,
                curves,
                offsets,
                tiles,
                notes,
                drawn: Cell::new((u8::MAX, 0)),
            },
        )
    }

    fn show(&self, snapshot: &Snapshot) {
        let now = snapshot.fade.as_ref().map_or((0, 0), |fade| {
            (
                1 + fade.live as u8 + fade.phase.len() as u8,
                fade.elapsed.to_bits() ^ snapshot.notes.skipped.is_some() as u64,
            )
        });
        if self.drawn.replace(now) != now {
            self.curves.queue_draw();
            self.offsets.queue_draw();
        }
        let [progress, gains, stretch, apart, tempos, matched] = &self.tiles;
        let notes = &snapshot.notes;
        let Some(fade) = snapshot.fade.as_ref() else {
            set_chip(&self.status, "IDLE", "dim");
            self.tiles.iter().for_each(|tile| tile.set("–", "", "dim"));
            self.notes[0].set_text("No crossfade yet in this session.");
            self.notes[1].set_text("");
            return;
        };
        let (text, kind) = match (fade.live, fade.phase.as_str()) {
            (true, "starting") => ("STARTING", "cyan"),
            (true, _) => ("FADING", "amber"),
            _ => ("LAST FADE", "dim"),
        };
        set_chip(&self.status, text, kind);
        progress.set(
            &format!("{:.1} / {:.1} s", fade.elapsed, fade.total),
            &fade
                .readiness
                .as_deref()
                .map_or(String::new(), |readiness| format!("next was {readiness}")),
            "",
        );
        gains.set(
            &format!("{:.2} / {:.2}", fade.gain_out, fade.gain_in),
            "gain, linear",
            if fade.live { "amber" } else { "" },
        );
        let sync = fade.sync.as_ref();
        match sync.and_then(|sync| sync.stretch_percent) {
            Some(percent) => stretch.set(
                &format!("{percent:+.1} %"),
                &match sync.and_then(|sync| sync.multiple) {
                    Some(m) if m > 1.5 => "counted double".to_owned(),
                    Some(m) if m < 0.75 => "counted half".to_owned(),
                    _ => String::new(),
                },
                "amber",
            ),
            None => stretch.set("–", "", "dim"),
        }
        match fade.beat_ms {
            Some(ms) => apart.set(
                &format!("{ms:+.0} ms"),
                if fade.beat_settled {
                    "measured"
                } else {
                    "clock settling"
                },
                match (fade.beat_settled, ms.abs() < 12.0) {
                    (false, _) => "dim",
                    (true, true) => "mint",
                    (true, false) => "red",
                },
            ),
            None => apart.set("–", "", "dim"),
        }
        match sync {
            Some(sync) => {
                tempos.set(
                    &format!(
                        "{:.1} → {}",
                        sync.bpm_in,
                        sync.bpm_out
                            .map_or("?".to_owned(), |bpm| format!("{bpm:.1}"))
                    ),
                    "BPM in → out",
                    "",
                );
                matched.set(
                    &sync.stage,
                    &if sync.nudging {
                        "nudging".to_owned()
                    } else {
                        format!("{} readings", sync.errors_ms.len())
                    },
                    if sync.stage == "matched" {
                        "mint"
                    } else {
                        "amber"
                    },
                );
            }
            None => {
                tempos.set("–", "", "dim");
                let (word, kind) = match (&notes.skipped, &notes.matched) {
                    (Some(_), _) => ("skipped", "red"),
                    (None, Some(_)) => ("matched", "mint"),
                    (None, None)
                        if snapshot
                            .dj
                            .settings
                            .is_some_and(|settings| settings.match_beats) =>
                    {
                        ("–", "dim")
                    }
                    (None, None) => ("off", "dim"),
                };
                matched.set(word, "", kind);
            }
        }
        self.notes[0].set_text(&match (&notes.skipped, &notes.matched) {
            (Some(why), _) => format!("beat match skipped: {why}"),
            (None, Some(matched)) => format!(
                "beat match: {matched}{}",
                notes
                    .held
                    .as_deref()
                    .and_then(|held| held.strip_prefix("holding the next track "))
                    .map_or(String::new(), |held| format!(" · held {held}"))
            ),
            (None, None) => notes.fade.clone().unwrap_or_default(),
        });
        self.notes[1].set_text(
            notes
                .summary
                .as_deref()
                .or(notes.started.as_deref())
                .unwrap_or(""),
        );
    }
}

impl Queue {
    fn new() -> (gtk::Box, Self) {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.add_css_class("k-dev-page");

        let now = card();
        now.append(&label(&["k-dev-h"], "IN THE PLAYER"));
        let grid = gtk::Grid::new();
        grid.set_column_spacing(14);
        grid.set_row_spacing(4);
        let keys = [
            "Title", "Id", "Uploader", "Length", "Numbers", "Uploaded", "Page", "Prepared",
        ];
        let mut row = 0;
        let video = keys.map(|key| {
            grid.attach(&label(&["k-dev-key"], key), 0, row, 1, 1);
            let value = label(&["k-dev-td"], "–");
            value.set_hexpand(true);
            value.set_selectable(true);
            grid.attach(&value, 1, row, 1, 1);
            row += 1;
            value
        });
        now.append(&grid);
        page.append(&now);

        let next = card();
        next.append(&label(&["k-dev-h"], "UP NEXT"));
        let table = gtk::Grid::new();
        table.set_column_spacing(12);
        table.set_row_spacing(6);
        for (column, heading) in ["#", "TRACK", "ID", "LEN", "STREAM", "BEATS", "ENTRY"]
            .into_iter()
            .enumerate()
        {
            table.attach(&label(&["k-dev-cap"], heading), column as i32, 0, 1, 1);
        }
        let rows = (0..ROWS)
            .map(|row| {
                let mut column = 0;
                [(); 7].map(|_| {
                    let cell = match column {
                        4 => chip(""),
                        _ => label(&["k-dev-td"], ""),
                    };
                    cell.set_xalign(0.0);
                    cell.set_halign(gtk::Align::Start);
                    if column == 1 {
                        cell.set_hexpand(true);
                        cell.set_halign(gtk::Align::Fill);
                    }
                    table.attach(&cell, column, row as i32 + 1, 1, 1);
                    column += 1;
                    cell
                })
            })
            .collect();
        next.append(&table);
        let warm = label(&["k-dev-note"], "");
        next.append(&warm);
        page.append(&next);
        (page, Self { video, rows, warm })
    }

    fn show(&self, snapshot: &Snapshot) {
        let [
            title,
            id,
            uploader,
            length,
            numbers,
            uploaded,
            page,
            prepared,
        ] = &self.video;
        match snapshot.video.as_ref() {
            Some(video) => {
                title.set_text(&video.title);
                id.set_text(&format!("{} ({})", video.id, video.platform));
                uploader.set_text(&format!("{} ({})", video.uploader, video.uploader_id));
                length.set_text(&video.duration.map_or("not known".into(), clock));
                numbers.set_text(&format!(
                    "{} views · {} likes{}",
                    video.views.map_or("?".into(), grouped),
                    video.likes.map_or("?".into(), grouped),
                    if video.live { " · live" } else { "" }
                ));
                uploaded.set_text(&video.uploaded);
                page.set_text(&format!(
                    "{}{}{}",
                    if video.dj_page {
                        "DJ track"
                    } else {
                        "video page"
                    },
                    if video.audio_only {
                        " · audio only"
                    } else {
                        ""
                    },
                    if video.downloaded {
                        " · downloaded"
                    } else {
                        ""
                    }
                ));
                prepared.set_text(&format!(
                    "stream {} · audio stream {}",
                    if video.prepared { "yes" } else { "no" },
                    if video.prepared_audio { "yes" } else { "no" }
                ));
            }
            None => self.video.iter().for_each(|value| value.set_text("–")),
        }
        for (index, row) in self.rows.iter().enumerate() {
            let [number, track, id, length, stream, beats, entry] = row;
            let Some(item) = snapshot.queue.get(index) else {
                row.iter().for_each(|cell| cell.set_visible(false));
                continue;
            };
            row.iter().for_each(|cell| cell.set_visible(true));
            number.set_text(&(index + 1).to_string());
            track.set_text(&format!("{} · {}", item.title, item.artist));
            id.set_text(&item.id);
            length.set_text(&item.duration.map_or("?".into(), short_clock));
            set_chip(
                stream,
                &item.stream.to_uppercase(),
                status_kind(&item.stream),
            );
            beats.set_text(&match item.beats.status.as_str() {
                "cached" => beats_text(&item.beats),
                "failed" => format!("failed: {}", beats_text(&item.beats)),
                other => other.to_owned(),
            });
            beats.set_css_classes(if item.beats.status == "cached" {
                &["k-dev-td"]
            } else {
                &["k-dev-td", "dim"]
            });
            entry.set_text(&match item.beats.entry.as_ref() {
                Some(found) if found.start > 0.0 => format!("{:.2} s", found.start),
                Some(_) => "start".to_owned(),
                None => "–".to_owned(),
            });
        }
        let dj = &snapshot.dj;
        self.warm.set_text(&if dj.on {
            format!(
                "{} queued · {} played · {} loaded ahead{}{}",
                dj.queued,
                dj.played,
                dj.warm.len(),
                dj.hovered
                    .as_deref()
                    .map_or(String::new(), |id| format!(" · pointer on {id}")),
                if dj.refilling { " · refilling" } else { "" }
            )
        } else {
            "The DJ is off: no queue.".to_owned()
        });
    }
}

impl System {
    fn new() -> (gtk::Box, Self) {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.add_css_class("k-dev-page");
        let process = card();
        process.append(&label(&["k-dev-h"], "PROCESS"));
        let (grid, tiles) = tiles(
            [
                "MEMORY",
                "CPU",
                "THREADS",
                "OPEN FILES",
                "PLAYERS",
                "UPTIME",
                "LOOP LAG",
                "WORST",
                "STALLS",
                "LOG",
            ],
            5,
        );
        let lag = canvas(90);
        process.append(&grid);
        process.append(&lag);
        page.append(&process);

        let lists = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        lists.set_homogeneous(true);
        let column = |heading: &str| {
            let frame = card();
            frame.append(&label(&["k-dev-h"], heading));
            let text = label(&["k-dev-mono"], "");
            text.set_ellipsize(gtk::pango::EllipsizeMode::None);
            text.set_wrap(true);
            text.set_selectable(true);
            text.set_yalign(0.0);
            frame.append(&text);
            lists.append(&frame);
            text
        };
        let (players, threads) = (column("PLAYERS"), column("THREADS"));
        page.append(&lists);

        let about = card();
        about.append(&label(&["k-dev-h"], "BUILD AND HOOKS"));
        let app = label(&["k-dev-mono"], "");
        app.set_ellipsize(gtk::pango::EllipsizeMode::None);
        app.set_wrap(true);
        app.set_selectable(true);
        about.append(&app);
        page.append(&about);
        (
            page,
            Self {
                tiles,
                lag,
                players,
                threads,
                app,
            },
        )
    }

    fn show(&self, snapshot: &Snapshot) {
        self.lag.queue_draw();
        let process = &snapshot.process;
        let [
            memory,
            cpu,
            threads,
            files,
            players,
            uptime,
            lag,
            worst,
            stalls,
            log,
        ] = &self.tiles;
        memory.set(
            &format!("{:.0} MB", process.rss_mb),
            &format!("peak {:.0} MB", process.peak_rss_mb),
            "",
        );
        cpu.set(
            &format!("{:.0} %", process.cpu_percent),
            "of one core",
            if process.cpu_percent > 80.0 {
                "red"
            } else {
                ""
            },
        );
        threads.set(&process.threads.to_string(), "", "");
        files.set(&process.open_files.to_string(), "", "");
        let playing = process
            .players
            .iter()
            .filter(|player| player.state == "playing")
            .count();
        players.set(
            &process.players.len().to_string(),
            &format!("{playing} playing"),
            if process.players.len() > 4 {
                "amber"
            } else {
                ""
            },
        );
        uptime.set(&short_clock(snapshot.uptime), "", "");
        let main_loop = &snapshot.main_loop;
        lag.set(
            &format!("{:.1} ms", main_loop.lag_ms_mean),
            "mean, last 60 s",
            "",
        );
        worst.set(
            &format!("{:.0} ms", main_loop.lag_ms_max),
            "last 60 s",
            if main_loop.lag_ms_max >= super::STALL_MS as f64 {
                "red"
            } else {
                ""
            },
        );
        stalls.set(
            &main_loop.stalls.to_string(),
            "100 ms late or more",
            if main_loop.stalls > 0 { "amber" } else { "" },
        );
        log.set(
            &format!("{} / {}", snapshot.notes.errors, snapshot.notes.warnings),
            "errors / warnings",
            if snapshot.notes.errors > 0 { "red" } else { "" },
        );
        let mut text = String::new();
        for player in &process.players {
            text.push_str(&format!(
                "{:<9} {:<10} {:<12} {:>5.0} s\n",
                player.role, player.state, player.id, player.age
            ));
        }
        self.players.set_text(text.trim_end());
        let mut text = String::new();
        for (name, count) in process.thread_names.iter().take(18) {
            text.push_str(&format!("{count:>3}  {name}\n"));
        }
        self.threads.set_text(text.trim_end());
        let app = &snapshot.app;
        let mut text = format!(
            "{} {} ({}) · pid {}\nGTK {} · {}\ndump: {}\n\
             this panel: snapshot {:.2} ms and labels {:.2} ms 5 times a second · waveforms {:.2} ms 20 times",
            app.app_id,
            app.version,
            app.profile,
            app.pid,
            app.gtk,
            app.gstreamer,
            super::dump_path().display(),
            snapshot.panel.collect_ms,
            snapshot.panel.show_ms,
            snapshot.panel.draw_ms
        );
        for (key, value) in &app.env {
            text.push_str(&format!("\n{key}={value}"));
        }
        self.app.set_text(&text);
    }
}

impl Log {
    fn new() -> (gtk::Box, Self) {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bar.add_css_class("k-dev-logbar");
        let level = gtk::DropDown::from_strings(&["Errors", "Warnings", "Info", "Debug", "All"]);
        level.set_selected(4);
        level.set_tooltip_text(Some("The least important lines shown"));
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Filter (text or module)"));
        search.set_hexpand(true);
        let follow = gtk::ToggleButton::with_label("Follow");
        follow.set_active(true);
        let clear = gtk::Button::with_label("Clear");
        let count = label(&["k-dev-note"], "");
        count.set_valign(gtk::Align::Center);
        bar.append(&level);
        bar.append(&search);
        bar.append(&count);
        bar.append(&follow);
        bar.append(&clear);
        let view = gtk::TextView::new();
        view.add_css_class("k-dev-log");
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        view.set_left_margin(12);
        view.set_right_margin(12);
        view.set_top_margin(6);
        view.set_bottom_margin(12);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        let buffer = view.buffer();
        for (name, color) in [
            ("ERROR", "#ff6b6b"),
            ("WARN", "#ffb454"),
            ("INFO", "#f3f4f6"),
            ("DEBUG", "#8a8f99"),
            ("TRACE", "#5d626b"),
            ("meta", "#5d626b"),
            ("target", "#5cccff"),
        ] {
            buffer.create_tag(Some(name), &[("foreground", &color)]);
        }
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_vexpand(true);
        scroller.set_child(Some(&view));
        page.append(&bar);
        page.append(&scroller);
        let log = Self {
            view,
            level,
            search,
            follow,
            count,
            seen: Cell::new(0),
        };
        clear.connect_clicked(glib::clone!(
            #[weak(rename_to = buffer)]
            log.view.buffer(),
            move |_| {
                logs::clear();
                buffer.set_text("");
            }
        ));
        (page, log)
    }

    /// Start over (the filter changed).
    fn reset(&self) {
        self.view.buffer().set_text("");
        self.seen.set(0);
    }

    /// Add the lines that came since the last look.
    fn show(&self) {
        let newest = logs::newest();
        if newest == self.seen.get() {
            return;
        }
        let least = match self.level.selected() {
            0 => log::Level::Error,
            1 => log::Level::Warn,
            2 => log::Level::Info,
            3 => log::Level::Debug,
            _ => log::Level::Trace,
        };
        let wanted = self.search.text().to_lowercase();
        let buffer = self.view.buffer();
        let mut end = buffer.end_iter();
        logs::since(self.seen.get(), |line| {
            if line.level > least {
                return;
            }
            let target = line
                .target
                .trim_start_matches("kinora::gui::")
                .trim_start_matches("kinora::");
            if !wanted.is_empty()
                && !line.text.to_lowercase().contains(&wanted)
                && !target.contains(&wanted)
            {
                return;
            }
            let level = line.level.as_str();
            buffer.insert_with_tags_by_name(&mut end, &format!("{:>9.3} ", line.at), &["meta"]);
            buffer.insert_with_tags_by_name(&mut end, &format!("{level:<5} "), &[level]);
            buffer.insert_with_tags_by_name(&mut end, &format!("{target}  "), &["target"]);
            buffer.insert_with_tags_by_name(&mut end, &line.text, &[level]);
            buffer.insert(&mut end, "\n");
        });
        self.seen.set(newest);
        if buffer.line_count() > LOG_LINES {
            let mut cut = buffer.iter_at_line(buffer.line_count() - LOG_LINES * 4 / 5);
            if let Some(cut) = cut.as_mut() {
                buffer.delete(&mut buffer.start_iter(), cut);
            }
        }
        self.count
            .set_text(&format!("{} lines", (buffer.line_count() - 1).max(0)));
        if self.follow.is_active() {
            let view = self.view.clone();
            glib::idle_add_local_once(move || {
                let mut end = view.buffer().end_iter();
                view.scroll_to_iter(&mut end, 0.0, false, 0.0, 1.0);
            });
        }
    }
}

impl Panel {
    pub fn new(main: &Window) -> Self {
        static STYLE: std::sync::Once = std::sync::Once::new();
        STYLE.call_once(|| {
            let provider = gtk::CssProvider::new();
            provider.load_from_string(include_str!("style.css"));
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
                );
            }
        });

        let window = adw::Window::builder()
            .title("Kinora Dev panel")
            .default_width(760)
            .default_height(900)
            .hide_on_close(true)
            .build();
        window.add_css_class("k-dev");
        let stack = adw::ViewStack::new();

        // Decks: what is set, both decks, the crossfade.
        let decks_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        decks_page.add_css_class("k-dev-page");
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let chips = [(); 6].map(|_| {
            let chip = chip("");
            strip.append(&chip);
            chip
        });
        decks_page.append(&strip);
        let (deck_a, card_a) = DeckCard::new("A");
        let (deck_b, card_b) = DeckCard::new("B");
        let (mix_card, mix) = Mix::new();
        decks_page.append(&deck_a);
        decks_page.append(&deck_b);
        decks_page.append(&mix_card);
        let (queue_page, queue) = Queue::new();
        let (system_page, system) = System::new();
        let (log_page, log) = Log::new();
        let scrolled = |child: &gtk::Box| {
            let scroller = gtk::ScrolledWindow::new();
            scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
            scroller.set_child(Some(child));
            scroller
        };
        stack.add_titled_with_icon(
            &scrolled(&decks_page),
            Some("decks"),
            "Decks",
            "media-optical-symbolic",
        );
        stack.add_titled_with_icon(
            &scrolled(&queue_page),
            Some("queue"),
            "Queue",
            "view-list-symbolic",
        );
        stack.add_titled_with_icon(
            &scrolled(&system_page),
            Some("system"),
            "System",
            "computer-symbolic",
        );
        stack.add_titled_with_icon(&log_page, Some("log"), "Log", "text-x-generic-symbolic");

        let header = adw::HeaderBar::new();
        let switcher = adw::ViewSwitcher::builder()
            .stack(&stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        header.set_title_widget(Some(&switcher));
        let dump = gtk::Button::from_icon_name("document-save-symbolic");
        dump.set_tooltip_text(Some("Write the JSON snapshot (dev-dump)"));
        header.pack_end(&dump);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&stack));
        window.set_content(Some(&view));

        let inner = Rc::new(Inner {
            main: main.downgrade(),
            window: window.clone(),
            stack,
            snapshot: RefCell::default(),
            timer: RefCell::default(),
            last: Cell::new(Instant::now()),
            looks: Cell::new(0),
            looked_at: RefCell::default(),
            drawn: Cell::default(),
            chips,
            decks: [card_a, card_b],
            mix,
            queue,
            system,
            log,
        });

        // What is drawn reads the last snapshot.
        for (index, deck) in inner.decks.iter().enumerate() {
            let letter = ["A", "B"][index];
            for (area, whole) in [(&deck.wave, false), (&deck.overview, true)] {
                let weak = Rc::downgrade(&inner);
                area.set_draw_func(move |_, cr, w, h| {
                    let Some(inner) = weak.upgrade() else { return };
                    let snapshot = inner.snapshot.borrow();
                    let deck = snapshot.decks.iter().find(|deck| deck.deck == letter);
                    inner.timed(|| {
                        if whole {
                            draw::overview(cr, w as f64, h as f64, deck);
                        } else {
                            draw::wave(cr, w as f64, h as f64, deck);
                        }
                    });
                });
            }
        }
        let weak = Rc::downgrade(&inner);
        inner.mix.curves.set_draw_func(move |_, cr, w, h| {
            let Some(inner) = weak.upgrade() else { return };
            let snapshot = inner.snapshot.borrow();
            draw::curves(cr, w as f64, h as f64, snapshot.fade.as_ref());
        });
        let weak = Rc::downgrade(&inner);
        inner.mix.offsets.set_draw_func(move |_, cr, w, h| {
            let Some(inner) = weak.upgrade() else { return };
            let snapshot = inner.snapshot.borrow();
            let note = offsets_note(&snapshot);
            super::FADE.with_borrow(|log| {
                // Until the new fade's first step, the last one's are there.
                let starting = snapshot
                    .fade
                    .as_ref()
                    .is_some_and(|fade: &Fade| fade.phase == "starting");
                if starting {
                    draw::offsets(cr, w as f64, h as f64, &super::FadeLog::default(), &note);
                } else {
                    draw::offsets(cr, w as f64, h as f64, log, &note);
                }
            });
        });
        inner.system.lag.set_draw_func(|_, cr, w, h| {
            super::HEART
                .with_borrow(|heart| draw::lag(cr, w as f64, h as f64, heart, logs::uptime()));
        });

        let weak = Rc::downgrade(&inner);
        dump.connect_clicked(move |_| {
            if let Some(main) = weak.upgrade().and_then(|inner| inner.main.upgrade()) {
                super::dump(&main);
            }
        });
        for reset in [true, false] {
            let weak = Rc::downgrade(&inner);
            let changed = move || {
                if let Some(inner) = weak.upgrade() {
                    inner.log.reset();
                    inner.log.show();
                }
            };
            if reset {
                inner.log.level.connect_selected_notify(move |_| changed());
            } else {
                inner.log.search.connect_search_changed(move |_| changed());
            }
        }

        // The same shortcut closes it.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&inner);
        keys.connect_key_pressed(move |_, key, _, state| {
            if super::is_shortcut(key, state)
                && let Some(main) = weak.upgrade().and_then(|inner| inner.main.upgrade())
            {
                super::toggle(&main);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(keys);

        // Only on screen does it look at anything.
        let weak = Rc::downgrade(&inner);
        window.connect_map(move |_| {
            let Some(inner) = weak.upgrade() else { return };
            peaks::set_wanted(true);
            inner.last.set(Instant::now());
            let weak = Rc::downgrade(&inner);
            let timer = glib::timeout_add_local(TICK, move || match weak.upgrade() {
                Some(inner) => {
                    inner.tick();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
            if let Some(old) = inner.timer.replace(Some(timer)) {
                old.remove();
            }
            inner.tick();
        });
        let weak = Rc::downgrade(&inner);
        window.connect_unmap(move |_| {
            peaks::set_wanted(false);
            super::COST.set(Default::default());
            if let Some(timer) = weak.upgrade().and_then(|inner| inner.timer.take()) {
                timer.remove();
            }
        });
        Self { inner }
    }

    pub fn window(&self) -> adw::Window {
        self.inner.window.clone()
    }

    /// The app's window closes: so does this.
    pub fn close(self) {
        self.inner.window.set_hide_on_close(false);
        self.inner.window.close();
    }
}

/// What the beats' graph says while it has nothing to draw.
fn offsets_note(snapshot: &Snapshot) -> String {
    let matching = snapshot
        .dj
        .settings
        .is_some_and(|settings| settings.match_beats);
    match (&snapshot.notes.skipped, snapshot.fade.as_ref()) {
        _ if !matching => "Match Beats is off".to_owned(),
        (Some(_), Some(_)) => "beat match skipped".to_owned(),
        (None, Some(fade)) if fade.live => "waiting for the beats".to_owned(),
        _ => "no matched fade yet".to_owned(),
    }
}

impl Inner {
    fn tick(&self) {
        let Some(main) = self.main.upgrade() else {
            return;
        };
        let now = Instant::now();
        let late = (now - self.last.replace(now)).saturating_sub(TICK);
        super::HEART.with_borrow_mut(|heart| heart.beat(late));
        // On another workspace nothing is drawn: nothing is looked at either.
        let suspended = self
            .window
            .surface()
            .and_then(|surface| surface.downcast::<gtk::gdk::Toplevel>().ok())
            .is_some_and(|toplevel| {
                toplevel
                    .state()
                    .contains(gtk::gdk::ToplevelState::SUSPENDED)
            });
        peaks::set_wanted(!suspended);
        if suspended {
            return;
        }
        let page = self.stack.visible_child_name();
        let page = page.as_deref().unwrap_or("decks");
        // Between two snapshots only the playheads move.
        let looks = self.looks.get().wrapping_add(1);
        self.looks.set(looks);
        if !looks.is_multiple_of(SNAPSHOT_EVERY) && *self.looked_at.borrow() == page {
            if page == "decks" {
                let positions = super::COLLECTOR.with_borrow(|collector| collector.positions());
                let mut snapshot = self.snapshot.borrow_mut();
                for (deck, position) in ["A", "B"].into_iter().zip(positions) {
                    if let (Some(position), Some(seated)) = (
                        position,
                        snapshot.decks.iter_mut().find(|seated| seated.deck == deck),
                    ) && (position - seated.position).abs() > 0.001
                    {
                        seated.position = position;
                        self.decks[(deck == "B") as usize].wave.queue_draw();
                    }
                }
            }
            return;
        }
        self.looked_at.replace(page.to_owned());
        let began = Instant::now();
        let snapshot = super::collect(
            &main,
            match page {
                "decks" => What::Decks,
                "queue" => What::Queue,
                "system" => What::System,
                _ => What::Log,
            },
        );
        let collected = began.elapsed();
        self.snapshot.replace(snapshot);
        let snapshot = self.snapshot.borrow();
        match page {
            "decks" => {
                self.show_chips(&snapshot);
                for (card, letter) in self.decks.iter().zip(["A", "B"]) {
                    card.show(snapshot.decks.iter().find(|deck| deck.deck == letter));
                }
                self.mix.show(&snapshot);
            }
            "queue" => self.queue.show(&snapshot),
            "system" => self.system.show(&snapshot),
            _ => self.log.show(),
        }
        // What the panel itself costs, smoothed: it is part of what it shows.
        let mut cost = super::COST.get();
        let smooth = |old: f64, new: Duration| old + (new.as_secs_f64() * 1000.0 - old) * 0.1;
        cost.collect_ms = smooth(cost.collect_ms, collected);
        cost.show_ms = smooth(cost.show_ms, began.elapsed() - collected);
        cost.draw_ms = smooth(cost.draw_ms, self.drawn.take() / SNAPSHOT_EVERY);
        super::COST.set(cost);
    }

    /// Time a drawing, for the panel's own cost.
    fn timed(&self, draw: impl FnOnce()) {
        let began = Instant::now();
        draw();
        self.drawn.set(self.drawn.get() + began.elapsed());
    }

    fn show_chips(&self, snapshot: &Snapshot) {
        let [dj, source, fade, matching, entry, health] = &self.chips;
        let on = snapshot.dj.on;
        set_chip(
            dj,
            if snapshot.dj.starting {
                "DJ STARTING"
            } else if on {
                "DJ ON"
            } else {
                "DJ OFF"
            },
            if on { "amber" } else { "dim" },
        );
        source.set_visible(on);
        if on {
            set_chip(
                source,
                &format!(
                    "{} · {} QUEUED",
                    snapshot.dj.source.as_deref().unwrap_or("?").to_uppercase(),
                    snapshot.dj.queued
                ),
                "dim",
            );
        }
        let settings = snapshot.dj.settings.unwrap_or_default();
        set_chip(
            fade,
            &format!("CROSSFADE {:.0} S", settings.crossfade.as_secs_f64()),
            "dim",
        );
        let switch = |on: bool| if on { "mint" } else { "dim" };
        set_chip(
            matching,
            if settings.match_beats {
                "MATCH BEATS ON"
            } else {
                "MATCH BEATS OFF"
            },
            switch(settings.match_beats),
        );
        set_chip(
            entry,
            if settings.start_on_beat {
                "START ON BEAT ON"
            } else {
                "START ON BEAT OFF"
            },
            switch(settings.start_on_beat),
        );
        set_chip(
            health,
            &format!(
                "{:.0} MB · {:.0} % CPU · {} PLAYERS",
                snapshot.process.rss_mb,
                snapshot.process.cpu_percent,
                snapshot.process.players.len()
            ),
            "dim",
        );
    }
}
