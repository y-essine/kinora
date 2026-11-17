// SPDX-License-Identifier: GPL-3.0-or-later

//! Kinora: the buffered part of the video, drawn ahead of the playhead on the
//! seek bar (like YouTube's lighter segment).
//!
//! Clapper doesn't report it, so we ask GStreamer: the adaptive demuxer
//! (`hlsdemux2` / `dashdemux2`, used for YouTube) knows how much it has
//! downloaded ahead (`current-level-time-*`); other streams answer a buffering
//! query. The seek bar is a `GtkScale` inside Clapper's seek bar, and GTK draws
//! its "fill level" as the `trough > fill` node (styled in style.css).

use std::time::Duration;

use clapper_gtk::prelude::AvExt;
use gst::prelude::*;
use gtk::glib;
use gtk::prelude::*;

/// How much the demuxer may download ahead. Clapper leaves it at 10 s, which
/// shows a stub of a buffered bar and stalls on a slow moment; 45 s is about
/// 35 MB at 1080p.
const MAX_BUFFERING: Duration = Duration::from_secs(45);
const TICK: Duration = Duration::from_millis(500);

/// Keep the seek bar's buffered segment up to date while `video` exists.
pub fn attach(video: &clapper_gtk::Video) {
    let video = video.downgrade();
    let mut demux: Option<glib::WeakRef<gst::Element>> = None;
    let mut scale: Option<glib::WeakRef<gtk::Scale>> = None;
    glib::timeout_add_local(TICK, move || {
        let Some(video) = video.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if !video.is_mapped() {
            return glib::ControlFlow::Continue;
        }
        let Some(player) = video.player() else {
            return glib::ControlFlow::Continue;
        };
        let Some(scale) = cached(&mut scale, || find_scale(video.upcast_ref())) else {
            return glib::ControlFlow::Continue;
        };
        let duration = player
            .queue()
            .and_then(|q| q.current_item())
            .map(|i| i.duration())
            .unwrap_or(0.0);
        let position = player.position();
        let pipeline = player.video_sink().and_then(|s| top_bin(&s));
        // A new video gets a new demuxer: look again when the cached one left.
        let ahead = pipeline.as_ref().and_then(|p| {
            let in_pipeline = demux
                .as_ref()
                .and_then(|d| d.upgrade())
                .is_some_and(|d| d.parent().is_some());
            if !in_pipeline {
                demux = find_demux(p).map(|d| {
                    let ns = MAX_BUFFERING.as_nanos() as u64;
                    log::debug!(
                        "Demuxer {}: high watermark {} s, max buffering {} s",
                        d.factory().map(|f| f.name().to_string()).unwrap_or_default(),
                        d.property::<u64>("high-watermark-time") / 1_000_000_000,
                        d.property::<u64>("max-buffering-time") / 1_000_000_000,
                    );
                    d.set_property("max-buffering-time", ns);
                    d.set_property("high-watermark-time", ns);
                    d.downgrade()
                });
            }
            match demux.as_ref().and_then(|d| d.upgrade()) {
                Some(d) => Some(demux_level(&d)),
                None => buffered_until(p, position).map(|end| end - position),
            }
        });
        let ahead = ahead.unwrap_or(0.0);
        log::debug!("Buffered: {ahead:.1} s ahead of {position:.1} s / {duration:.0} s");
        if duration <= 0.0 || ahead <= 0.0 {
            scale.set_show_fill_level(false);
            return glib::ControlFlow::Continue;
        }
        let adj = scale.adjustment();
        // Clapper sets the range once the controls have been on screen.
        if adj.upper() <= adj.lower() {
            return glib::ControlFlow::Continue;
        }
        let end = ((position + ahead) / duration).clamp(0.0, 1.0);
        scale.set_restrict_to_fill_level(false);
        scale.set_fill_level(adj.lower() + end * (adj.upper() - adj.lower()));
        scale.set_show_fill_level(true);
        glib::ControlFlow::Continue
    });
}

/// Seconds loaded ahead of `player`'s playhead, when anything can tell (for a
/// bar of our own: see `gui/wavy_bar.rs`).
pub fn buffered_ahead(player: &clapper::Player) -> Option<f64> {
    let pipeline = player.audio_sink().and_then(|s| top_bin(&s))?;
    let ahead = match find_demux(&pipeline) {
        Some(demux) => demux_level(&demux),
        None => {
            let position = player.position();
            buffered_until(&pipeline, position)? - position
        }
    };
    log::debug!("Buffered (audio): {ahead:.1} s ahead");
    (ahead > 0.0).then_some(ahead)
}

fn cached<T: IsA<glib::Object>>(
    slot: &mut Option<glib::WeakRef<T>>,
    find: impl FnOnce() -> Option<T>,
) -> Option<T> {
    if let Some(v) = slot.as_ref().and_then(|w| w.upgrade()) {
        return Some(v);
    }
    let v = find()?;
    *slot = Some(v.downgrade());
    Some(v)
}

/// The `GtkScale` inside Clapper's seek bar.
fn find_scale(root: &gtk::Widget) -> Option<gtk::Scale> {
    fn walk(w: &gtk::Widget, in_seek_bar: bool) -> Option<gtk::Scale> {
        let in_seek_bar = in_seek_bar || w.type_().name() == "ClapperGtkSeekBar";
        if in_seek_bar {
            if let Some(s) = w.downcast_ref::<gtk::Scale>() {
                return Some(s.clone());
            }
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            if let Some(s) = walk(&c, in_seek_bar) {
                return Some(s);
            }
            child = c.next_sibling();
        }
        None
    }
    walk(root, false)
}

fn top_bin(element: &gst::Element) -> Option<gst::Element> {
    let mut top: gst::Object = element.clone().upcast();
    while let Some(parent) = top.parent() {
        top = parent;
    }
    top.downcast::<gst::Element>().ok().filter(|t| t != element)
}

fn find_demux(pipeline: &gst::Element) -> Option<gst::Element> {
    let bin = pipeline.downcast_ref::<gst::Bin>()?;
    bin.iterate_recurse().into_iter().flatten().find(|e| {
        e.factory()
            .is_some_and(|f| matches!(f.name().as_str(), "hlsdemux2" | "dashdemux2"))
    })
}

/// Seconds downloaded ahead of the playhead: the smaller of audio and video
/// (a track that isn't there reports 0 and is ignored).
fn demux_level(demux: &gst::Element) -> f64 {
    // "None" (u64::MAX) once the whole track is downloaded.
    let level = |name: &str| match demux.property::<u64>(name) {
        u64::MAX => f64::INFINITY,
        ns => ns as f64 / 1e9,
    };
    let (v, a) = (level("current-level-time-video"), level("current-level-time-audio"));
    match (v > 0.0, a > 0.0) {
        (true, true) => v.min(a),
        (true, false) => v,
        (false, true) => a,
        (false, false) => 0.0,
    }
}

/// Where the buffered range around `position` ends (seconds), for streams
/// buffered by queue2/downloadbuffer.
fn buffered_until(pipeline: &gst::Element, position: f64) -> Option<f64> {
    let mut q = gst::query::Buffering::new(gst::Format::Time);
    if !pipeline.query(&mut q) {
        return None;
    }
    let pos = gst::ClockTime::from_nseconds((position * 1e9) as u64);
    q.ranges().into_iter().find_map(|(start, stop)| {
        let (start, stop) = (start.value(), stop.value());
        (start >= 0 && start as u64 <= pos.nseconds() && stop as u64 > pos.nseconds())
            .then(|| stop as f64 / 1e9)
    })
}

