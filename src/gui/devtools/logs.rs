// SPDX-License-Identifier: GPL-3.0-or-later

//! The Dev panel's log: the app's logger, kept as it was, plus the last
//! [`KEPT`] lines in memory, and a few facts read off the DJ's lines as they
//! go by (why a beat match was skipped, which analysis failed), so that
//! `dj.rs` needs no extra bookkeeping for the panel.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use serde::Serialize;

/// How many lines are kept.
pub const KEPT: usize = 4000;

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Seconds since the logger was installed (the app's start).
pub fn uptime() -> f64 {
    START.elapsed().as_secs_f64()
}

#[derive(Clone)]
pub struct Line {
    pub seq: u64,
    /// Seconds since the app started.
    pub at: f64,
    pub level: log::Level,
    pub target: String,
    pub text: String,
}

/// What the DJ's log lines said about the fade that runs (or ran last) and
/// about the analysis of tracks.
#[derive(Default, Clone, Serialize)]
pub struct Notes {
    /// "crossfade to ID (title), planned over 6.0 s", or the one asked by hand.
    pub fade: Option<String>,
    /// "started after 0.1 s (warm), from its entry at …; fading over 6.0 s".
    pub started: Option<String>,
    /// "126.0 BPM into 128.0 BPM: stretch +1.6 %".
    pub matched: Option<String>,
    /// Why the fade was not beat-matched.
    pub skipped: Option<String>,
    /// "holding the next track 212 ms for the beat (…)".
    pub held: Option<String>,
    /// How well the beats were together, said when the fade ended.
    pub summary: Option<String>,
    /// Tracks whose beats could not be worked out, and why.
    pub analysis_failed: BTreeMap<String, String>,
    pub warnings: u64,
    pub errors: u64,
}

struct Ring {
    lines: VecDeque<Line>,
    next: u64,
    notes: Notes,
}

static RING: LazyLock<Mutex<Ring>> = LazyLock::new(|| {
    Mutex::new(Ring {
        lines: VecDeque::with_capacity(KEPT),
        next: 1,
        notes: Notes::default(),
    })
});

fn note(notes: &mut Notes, text: &str) {
    let Some(text) = text.strip_prefix("DJ: ") else {
        return;
    };
    let own = |s: &str| Some(s.trim_start_matches([':', ' ']).to_owned());
    if text.starts_with("crossfade to ") || text.contains(" by hand: crossfade to ") {
        let failed = std::mem::take(&mut notes.analysis_failed);
        *notes = Notes {
            fade: Some(text.to_owned()),
            analysis_failed: failed,
            warnings: notes.warnings,
            errors: notes.errors,
            ..Notes::default()
        };
    } else if let Some(rest) = text.strip_prefix("beat match skipped") {
        notes.skipped = own(rest);
    } else if let Some(rest) = text.strip_prefix("beat match given up") {
        notes.skipped = Some(format!("given up: {}", rest.trim_start_matches([':', ' '])));
    } else if let Some(rest) = text.strip_prefix("beat match: ") {
        if rest.contains(" BPM into ") {
            notes.matched = own(rest);
        } else {
            notes.summary = own(rest);
        }
    } else if text.starts_with("holding the next track") {
        notes.held = Some(text.to_owned());
    } else if text.starts_with("started after") {
        notes.started = Some(text.to_owned());
    } else if let Some(rest) = text.strip_prefix("could not decode ")
        && let Some((id, why)) = rest.split_once(" for its beats: ")
    {
        notes.analysis_failed.insert(id.to_owned(), why.to_owned());
    }
}

struct Tee {
    inner: env_logger::Logger,
}

impl log::Log for Tee {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if !self.inner.matches(record) {
            return;
        }
        self.inner.log(record);
        let text = record.args().to_string();
        let mut ring = RING.lock().unwrap();
        match record.level() {
            log::Level::Error => ring.notes.errors += 1,
            log::Level::Warn => ring.notes.warnings += 1,
            _ => {}
        }
        if record.target().ends_with("::dj") {
            note(&mut ring.notes, &text);
        }
        if ring.lines.len() >= KEPT {
            ring.lines.pop_front();
        }
        let seq = ring.next;
        ring.next += 1;
        ring.lines.push_back(Line {
            seq,
            at: uptime(),
            level: record.level(),
            target: record.target().to_owned(),
            text,
        });
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Install `inner` (the logger the app would have used) with the ring
/// around it. What `RUST_LOG` lets through is what is kept.
pub fn install(inner: env_logger::Logger) {
    LazyLock::force(&START);
    let level = inner.filter();
    if log::set_boxed_logger(Box::new(Tee { inner })).is_ok() {
        log::set_max_level(level);
    }
}

/// The number of the newest line (0: none yet).
pub fn newest() -> u64 {
    RING.lock().unwrap().next - 1
}

/// Every kept line after number `after`, oldest first.
pub fn since(after: u64, mut each: impl FnMut(&Line)) {
    let ring = RING.lock().unwrap();
    let skip = ring
        .lines
        .front()
        .map_or(0, |first| (after + 1).saturating_sub(first.seq) as usize);
    ring.lines.iter().skip(skip).for_each(&mut each);
}

/// The last `count` lines.
pub fn tail(count: usize) -> Vec<Line> {
    let ring = RING.lock().unwrap();
    let skip = ring.lines.len().saturating_sub(count);
    ring.lines.iter().skip(skip).cloned().collect()
}

pub fn notes() -> Notes {
    RING.lock().unwrap().notes.clone()
}

pub fn clear() {
    RING.lock().unwrap().lines.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_djs_lines_are_read() {
        let mut notes = Notes::default();
        note(
            &mut notes,
            "DJ: crossfade to abc (Song), planned over 6.0 s",
        );
        note(
            &mut notes,
            "DJ: beat match: 126.0 BPM into 128.0 BPM: stretch +1.6 %",
        );
        note(
            &mut notes,
            "DJ: beat match: 12 readings, beats 4 ms apart on average",
        );
        assert_eq!(
            notes.matched.as_deref(),
            Some("126.0 BPM into 128.0 BPM: stretch +1.6 %")
        );
        assert!(notes.summary.as_deref().unwrap().starts_with("12 readings"));
        note(
            &mut notes,
            "DJ: could not decode xyz for its beats: timed out",
        );
        note(
            &mut notes,
            "DJ: Next by hand: crossfade to def (Other) over the 6 s setting",
        );
        assert!(notes.matched.is_none());
        assert_eq!(notes.analysis_failed["xyz"], "timed out");
        note(
            &mut notes,
            "DJ: beat match skipped (90.0 BPM into 128.0 BPM): the tempos are 11.1 % apart",
        );
        assert!(notes.skipped.as_deref().unwrap().starts_with("(90.0 BPM"));
    }
}
