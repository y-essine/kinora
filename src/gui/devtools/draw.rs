// SPDX-License-Identifier: GPL-3.0-or-later

//! What the Dev panel draws itself: the decks' waveforms, the crossfade's
//! curves, the beats' distance and the main loop's lag. All of it reads
//! numbers that are already there; nothing is worked out here but pixels.

use gtk::cairo::Context;

use super::peaks::{self, PER_SECOND};
use super::state::{Deck, Fade};
use super::{FadeLog, Heart};

type Rgb = (f64, f64, f64);

const WELL: Rgb = (0.043, 0.047, 0.059);
const TEXT: Rgb = (0.953, 0.957, 0.965);
/// The bands: kick, body, air.
const LOW: Rgb = (0.93, 0.33, 0.20);
const MID: Rgb = (1.0, 0.706, 0.329);
const HIGH: Rgb = (0.93, 0.96, 1.0);
pub const AMBER: Rgb = MID;
pub const CYAN: Rgb = (0.36, 0.80, 1.0);
const MINT: Rgb = (0.34, 0.86, 0.64);
const RED: Rgb = (1.0, 0.42, 0.42);

/// Seconds of the track across the scrolling waveform, and where in it the
/// playhead stands.
const SPAN: f64 = 12.0;
const PLAYHEAD_AT: f64 = 0.3;

fn color(cr: &Context, (r, g, b): Rgb, alpha: f64) {
    cr.set_source_rgba(r, g, b, alpha);
}

fn rounded(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
    cr.close_path();
}

fn well(cr: &Context, w: f64, h: f64) {
    rounded(cr, 0.0, 0.0, w, h, 6.0);
    cr.clip();
    color(cr, WELL, 1.0);
    let _ = cr.paint();
}

fn text(cr: &Context, x: f64, y: f64, size: f64, rgb: Rgb, alpha: f64, what: &str) {
    cr.select_font_face(
        "monospace",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    cr.set_font_size(size);
    color(cr, rgb, alpha);
    cr.move_to(x, y);
    let _ = cr.show_text(what);
}

fn text_width(cr: &Context, size: f64, what: &str) -> f64 {
    cr.set_font_size(size);
    cr.text_extents(what).map_or(0.0, |extents| extents.width())
}

fn centered(cr: &Context, w: f64, h: f64, what: &str) {
    cr.select_font_face(
        "monospace",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    let width = text_width(cr, 11.0, what);
    text(cr, (w - width) / 2.0, h / 2.0 + 4.0, 11.0, TEXT, 0.35, what);
}

fn vline(cr: &Context, x: f64, top: f64, bottom: f64, width: f64) {
    cr.rectangle((x - width / 2.0).round(), top, width, bottom - top);
}

/// A deck's scrolling waveform: [`SPAN`] seconds around the playhead, the
/// bands in three colours, the beats, the entry, what is buffered.
pub fn wave(cr: &Context, w: f64, h: f64, deck: Option<&Deck>) {
    well(cr, w, h);
    let Some(deck) = deck else {
        return centered(cr, w, h, "empty deck");
    };
    let pps = w / SPAN;
    let x0 = (w * PLAYHEAD_AT).round();
    let left = deck.position - x0 / pps;
    let right = left + SPAN;
    let x_of = |t: f64| (t - left) * pps;
    let mid = h / 2.0;
    let half = mid - 9.0;

    // Before the track and after its end: nothing to play.
    color(cr, (0.0, 0.0, 0.0), 0.45);
    if left < 0.0 {
        cr.rectangle(0.0, 0.0, x_of(0.0), h);
    }
    if let Some(duration) = deck.duration.filter(|d| *d < right) {
        cr.rectangle(x_of(duration), 0.0, w - x_of(duration), h);
    }
    let _ = cr.fill();

    // The beats: a faint line each, every fourth one stronger.
    if let Some(grid) = deck.grid.as_ref().filter(|grid| grid.bpm > 30.0) {
        let period = 60.0 / grid.bpm;
        let first = ((left.max(0.0) - grid.anchor) / period).ceil() as i64;
        for strong in [false, true] {
            let mut n = first;
            loop {
                let t = grid.anchor + n as f64 * period;
                if t > right || deck.duration.is_some_and(|d| t > d) {
                    break;
                }
                if (n.rem_euclid(4) == 0) == strong {
                    vline(cr, x_of(t), 0.0, h, 1.0);
                }
                n += 1;
            }
            color(cr, TEXT, if strong { 0.26 } else { 0.09 });
            let _ = cr.fill();
        }
    }

    let known = peaks::with(&deck.id, |track| {
        let Some(track) = track else {
            return false;
        };
        let from = ((left * PER_SECOND).floor().max(0.0)) as usize;
        let to = (((right * PER_SECOND).ceil().max(0.0)) as usize).min(track.bins.len());
        if from >= to {
            return false;
        }
        let bar = (pps / PER_SECOND).max(1.0);
        let mut any = false;
        // One pass and one fill per layer: the peak's outline, then the
        // bands, the tallest behind.
        let layers: [(Rgb, f64, fn(&peaks::Bin) -> u8); 4] = [
            (MID, 0.16, |bin| bin.amp),
            (LOW, 0.95, |bin| bin.low),
            (MID, 0.95, |bin| bin.mid),
            (HIGH, 0.92, |bin| bin.high),
        ];
        for (index, (rgb, alpha, level)) in layers.into_iter().enumerate() {
            for (offset, bin) in track.bins[from..to].iter().enumerate() {
                if !bin.known {
                    continue;
                }
                any = true;
                // The bands narrow towards the front, like a record's groove.
                let scale = [1.0, 1.0, 0.7, 0.38][index];
                let height = (level(bin) as f64 / 255.0 * half * scale).max(0.5);
                let x = x_of((from + offset) as f64 / PER_SECOND);
                cr.rectangle(x, mid - height, bar, height * 2.0);
            }
            color(cr, rgb, alpha);
            let _ = cr.fill();
        }
        any
    });

    // What has played is dimmed.
    color(cr, WELL, 0.55);
    cr.rectangle(0.0, 0.0, x0, h);
    let _ = cr.fill();

    // Buffered ahead (mint, along the bottom) and decoded (a tick on it).
    if let Some(ahead) = deck.buffered_ahead.filter(|ahead| *ahead > 0.0) {
        color(cr, MINT, 0.9);
        cr.rectangle(x0, h - 3.0, (ahead * pps).min(w - x0), 3.0);
        let _ = cr.fill();
    }
    if let Some(flow) = deck.decoded_to.filter(|flow| *flow > deck.position) {
        color(cr, AMBER, 0.9);
        vline(cr, x_of(flow), h - 8.0, h, 2.0);
        let _ = cr.fill();
    }

    // "Start on the Beat": where the track comes in.
    let entry = deck
        .beats
        .entry
        .as_ref()
        .map(|entry| entry.start)
        .filter(|start| *start > 0.0)
        .or(deck.entry_target);
    if let Some(entry) = entry.filter(|entry| (left..right).contains(entry)) {
        let x = x_of(entry);
        color(cr, CYAN, 0.95);
        vline(cr, x, 0.0, h, 2.0);
        cr.rectangle(x, 0.0, 38.0, 12.0);
        let _ = cr.fill();
        text(cr, x + 4.0, 9.5, 9.0, WELL, 1.0, "ENTRY");
    }

    // Seconds along the top.
    let step = 2.0;
    let mut t = (left.max(0.0) / step).ceil() * step;
    while t < right {
        let label = format!("{}:{:02}", (t / 60.0) as u32, (t % 60.0) as u32);
        text(cr, x_of(t) + 3.0, h - 6.0, 8.5, TEXT, 0.28, &label);
        t += step;
    }

    // The playhead.
    color(cr, (0.0, 0.0, 0.0), 0.5);
    vline(cr, x0, 0.0, h, 4.0);
    let _ = cr.fill();
    color(cr, TEXT, 1.0);
    vline(cr, x0, 0.0, h, 2.0);
    cr.move_to(x0 - 5.0, 0.0);
    cr.line_to(x0 + 5.0, 0.0);
    cr.line_to(x0, 6.0);
    cr.close_path();
    let _ = cr.fill();

    if !known {
        let what = match deck.waveform.source.as_str() {
            "decoding" => "decoding the waveform…",
            _ if deck.role == "queued" => "no player yet",
            _ if deck.role == "video" => "no waveform: only the DJ's decks keep their audio",
            _ => "no audio seen here yet",
        };
        centered(cr, w + x0, h, what);
    }
}

/// The whole track in one strip: what is known of it, where it is, what is
/// buffered, where it comes in, and what the scrolling view shows.
pub fn overview(cr: &Context, w: f64, h: f64, deck: Option<&Deck>) {
    well(cr, w, h);
    let Some(deck) = deck else {
        return;
    };
    let Some(duration) = deck.duration.filter(|d| *d > 1.0) else {
        return;
    };
    let x_of = |t: f64| (t / duration).clamp(0.0, 1.0) * w;
    let now = x_of(deck.position);
    peaks::with(&deck.id, |track| {
        let Some(track) = track else {
            return;
        };
        let per_px = duration * PER_SECOND / w;
        for played in [true, false] {
            let (from, to) = if played {
                (0, now as usize)
            } else {
                (now as usize, w as usize)
            };
            for px in from..to {
                let first = (px as f64 * per_px) as usize;
                let last = (((px + 1) as f64 * per_px) as usize).min(track.bins.len());
                if first >= last {
                    break;
                }
                let peak = track.bins[first..last]
                    .iter()
                    .filter(|bin| bin.known)
                    .map(|bin| bin.amp.max(bin.low))
                    .max();
                if let Some(peak) = peak {
                    let height = (peak as f64 / 255.0 * (h / 2.0 - 2.0)).max(0.5);
                    cr.rectangle(px as f64, h / 2.0 - height, 1.0, height * 2.0);
                }
            }
            color(cr, AMBER, if played { 0.32 } else { 0.85 });
            let _ = cr.fill();
        }
    });
    if let Some(ahead) = deck.buffered_ahead.filter(|ahead| *ahead > 0.0) {
        color(cr, MINT, 0.9);
        cr.rectangle(now, h - 2.0, x_of(deck.position + ahead) - now, 2.0);
        let _ = cr.fill();
    }
    if let Some(entry) = deck.beats.entry.as_ref().filter(|entry| entry.start > 0.0) {
        color(cr, CYAN, 0.95);
        vline(cr, x_of(entry.start), 0.0, h, 2.0);
        let _ = cr.fill();
    }
    // What the scrolling view shows.
    let view = SPAN / duration * w;
    color(cr, TEXT, 0.10);
    cr.rectangle(now - view * PLAYHEAD_AT, 0.0, view, h);
    let _ = cr.fill();
    color(cr, TEXT, 1.0);
    vline(cr, now, 0.0, h, 2.0);
    let _ = cr.fill();
}

/// The crossfade: both gains over its length, and where it is.
pub fn curves(cr: &Context, w: f64, h: f64, fade: Option<&Fade>) {
    well(cr, w, h);
    let (pad, top) = (8.0, 16.0);
    let (gw, gh) = (w - pad * 2.0, h - top - pad);
    let point = |p: f64, gain: f64| (pad + p * gw, top + (1.0 - gain) * gh);
    let progress = fade
        .filter(|fade| fade.total > 0.0 && fade.phase != "starting")
        .map(|fade| (fade.elapsed / fade.total).clamp(0.0, 1.0));
    let live = fade.is_some_and(|fade| fade.live);
    let strength = if live { 1.0 } else { 0.45 };
    // The plan's curves are equal power: cos out, sin in.
    for (rgb, gain) in [
        (AMBER, f64::cos as fn(f64) -> f64),
        (CYAN, f64::sin as fn(f64) -> f64),
    ] {
        let curve = |cr: &Context, until: f64| {
            let steps = 48;
            for step in 0..=steps {
                let p = until * step as f64 / steps as f64;
                let (x, y) = point(p, gain(p * std::f64::consts::FRAC_PI_2));
                if step == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
        };
        if let Some(progress) = progress {
            curve(cr, progress);
            cr.line_to(point(progress, 0.0).0, point(0.0, 0.0).1);
            cr.line_to(pad, point(0.0, 0.0).1);
            cr.close_path();
            color(cr, rgb, 0.16 * strength);
            let _ = cr.fill();
        }
        curve(cr, 1.0);
        color(cr, rgb, 0.9 * strength);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
        if let Some(progress) = progress.filter(|_| live) {
            let (x, y) = point(progress, gain(progress * std::f64::consts::FRAC_PI_2));
            cr.arc(x, y, 4.0, 0.0, std::f64::consts::TAU);
            color(cr, rgb, 1.0);
            let _ = cr.fill();
        }
    }
    if let Some(progress) = progress {
        color(cr, TEXT, 0.8 * strength);
        vline(cr, point(progress, 0.0).0, top - 4.0, h - pad, 1.0);
        let _ = cr.fill();
    }
    text(cr, pad, 11.0, 9.0, AMBER, 0.9, "OUT");
    text(cr, w - pad - 14.0, 11.0, 9.0, CYAN, 0.9, "IN");
    let label = match fade {
        None => "no crossfade yet".to_owned(),
        Some(fade) if fade.phase == "starting" => "waiting for the next track".to_owned(),
        Some(fade) if fade.live => format!("{:.1} / {:.1} s", fade.elapsed, fade.total),
        Some(fade) => format!("last fade · {:.1} s", fade.total),
    };
    cr.select_font_face(
        "monospace",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    let width = text_width(cr, 9.0, &label);
    text(cr, (w - width) / 2.0, 11.0, 9.0, TEXT, 0.55, &label);
}

/// "Match Beats": how far the beats were apart over the fade, in
/// milliseconds. Grey while the incoming player's clock settles.
pub fn offsets(cr: &Context, w: f64, h: f64, log: &FadeLog, note: &str) {
    well(cr, w, h);
    let readings = log.samples.iter().filter(|s| s.beat_ms.is_some()).count();
    if readings == 0 || log.total <= 0.0 {
        return centered(cr, w, h, note);
    }
    let worst = log
        .samples
        .iter()
        .filter_map(|s| s.beat_ms)
        .fold(0.0f32, |worst, ms| worst.max(ms.abs())) as f64;
    let range = (worst * 1.15).max(30.0);
    let (pad, mid) = (8.0, h / 2.0);
    let y_of = |ms: f64| mid - ms / range * (mid - 6.0);
    let x_of = |t: f64| pad + (t / log.total).clamp(0.0, 1.0) * (w - pad * 2.0);
    // Within 12 ms nothing is nudged.
    color(cr, MINT, 0.10);
    cr.rectangle(0.0, y_of(12.0), w, y_of(-12.0) - y_of(12.0));
    let _ = cr.fill();
    color(cr, TEXT, 0.2);
    cr.rectangle(0.0, mid.round(), w, 1.0);
    let _ = cr.fill();
    for settled in [false, true] {
        let mut pen_down = false;
        for sample in &log.samples {
            let Some(ms) = sample.beat_ms.filter(|_| sample.settled == settled) else {
                pen_down = false;
                continue;
            };
            let (x, y) = (x_of(sample.t as f64), y_of(ms as f64));
            if pen_down {
                cr.line_to(x, y);
            } else {
                cr.move_to(x, y);
            }
            pen_down = true;
        }
        if settled {
            color(cr, AMBER, 1.0);
        } else {
            color(cr, TEXT, 0.4);
        }
        cr.set_line_width(1.6);
        let _ = cr.stroke();
    }
    text(cr, 6.0, 11.0, 8.5, TEXT, 0.4, &format!("+{range:.0} ms"));
    text(cr, 6.0, h - 5.0, 8.5, TEXT, 0.4, &format!("-{range:.0} ms"));
    text(cr, w - 92.0, 11.0, 8.5, TEXT, 0.4, "beats apart");
}

/// The main loop's lag over the last minute: how late each timer came.
pub fn lag(cr: &Context, w: f64, h: f64, heart: &Heart, now: f64) {
    well(cr, w, h);
    let worst = heart
        .lags
        .iter()
        .fold(0.0f32, |worst, (_, lag)| worst.max(*lag)) as f64;
    let range = (worst * 1.1).max(50.0);
    let y_of = |ms: f64| h - 4.0 - (ms / range).min(1.0) * (h - 18.0);
    let x_of = |at: f64| w - (now - at) / super::HEART_KEPT * w;
    for (ms, rgb) in [(16.7, MINT), (super::STALL_MS as f64, RED)] {
        if ms < range {
            color(cr, rgb, 0.35);
            cr.rectangle(0.0, y_of(ms).round(), w, 1.0);
            let _ = cr.fill();
            text(cr, 4.0, y_of(ms) - 3.0, 8.5, rgb, 0.7, &format!("{ms:.0} ms"));
        }
    }
    for stalled in [false, true] {
        for (at, lag) in heart.lags.iter() {
            if (*lag >= super::STALL_MS) == stalled {
                let y = y_of(*lag as f64);
                cr.rectangle(x_of(*at), y, 1.5, h - 4.0 - y + 1.0);
            }
        }
        color(cr, if stalled { RED } else { AMBER }, 0.9);
        let _ = cr.fill();
    }
    text(cr, w - 112.0, 11.0, 8.5, TEXT, 0.4, "main loop, last 60 s");
}
