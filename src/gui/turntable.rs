// SPDX-License-Identifier: GPL-3.0-or-later

//! Kinora: the DJ's record player (see `docs/design.md`, "DJ").
//!
//! A square deck: a platter, a record whose label is the track's artwork, and
//! a tonearm. While `playing`, the label turns and the arm lies on the record,
//! moving inwards with `progress`; paused, the record eases to a stop and the
//! arm goes back to its rest. A new deck's record drops in, so a change of
//! track reads as a change of record.
//!
//! Everything is drawn from a few gradients and one texture; only the label's
//! transform changes from frame to frame, and frames are asked for only while
//! something moves. With `still` (the "Reduce effects" setting) nothing moves:
//! the arm is simply on the record or off it.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

glib::wrapper! {
    pub struct Turntable(ObjectSubclass<imp::Turntable>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

// The deck's geometry, in parts of its side.
const RECORD_X: f32 = 0.45;
const RECORD_Y: f32 = 0.54;
const RECORD_R: f32 = 0.40;
const PLATTER_R: f32 = 0.415;
/// The label's radius, in parts of the record's.
const LABEL: f32 = 0.42;
const PIVOT_X: f32 = 0.885;
const PIVOT_Y: f32 = 0.17;
/// The arm's angles from straight down, in degrees: at rest beside the
/// platter, on the first groove, on the last one.
const ARM_REST: f64 = -2.0;
const ARM_FIRST: f64 = 8.5;
const ARM_LAST: f64 = 29.7;

/// Degrees per second while playing: a calm turn every five seconds (a real
/// 33 rpm would make the artwork unreadable).
const SPIN: f64 = 72.0;
/// How quickly the record takes up speed and loses it, in seconds.
const SPIN_EASE: f64 = 0.45;
const ARM_EASE: f64 = 0.16;
/// How long a new record takes to drop in, in seconds.
const DROP: f64 = 0.35;

impl Turntable {
    /// The record's place in the widget: its centre and its radius.
    pub fn record(&self) -> (f64, f64, f64) {
        let (w, h) = (self.width() as f64, self.height() as f64);
        let s = w.min(h);
        (
            (w - s) / 2.0 + s * RECORD_X as f64,
            (h - s) / 2.0 + s * RECORD_Y as f64,
            s * RECORD_R as f64,
        )
    }
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> gdk::RGBA {
    gdk::RGBA::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a)
}

fn disc(x: f32, y: f32, r: f32) -> gsk::RoundedRect {
    gsk::RoundedRect::from_rect(graphene::Rect::new(x - r, y - r, 2.0 * r, 2.0 * r), r)
}

fn fill_disc(snapshot: &gtk::Snapshot, x: f32, y: f32, r: f32, color: &gdk::RGBA) {
    let shape = disc(x, y, r);
    snapshot.push_rounded_clip(&shape);
    snapshot.append_color(color, shape.bounds());
    snapshot.pop();
}

fn fill_rounded(snapshot: &gtk::Snapshot, rect: graphene::Rect, r: f32, color: &gdk::RGBA) {
    let shape = gsk::RoundedRect::from_rect(rect, r);
    snapshot.push_rounded_clip(&shape);
    snapshot.append_color(color, &rect);
    snapshot.pop();
}

fn ring(snapshot: &gtk::Snapshot, x: f32, y: f32, r: f32, width: f32, color: gdk::RGBA) {
    snapshot.append_border(&disc(x, y, r), &[width; 4], &[color; 4]);
}

mod imp {
    use std::cell::{Cell, RefCell};

    use super::*;

    #[derive(glib::Properties, Default)]
    #[properties(wrapper_type = super::Turntable)]
    pub struct Turntable {
        /// The picture on the record's label.
        #[property(get, set = Self::set_artwork, nullable)]
        artwork: RefCell<Option<gdk::Paintable>>,
        #[property(get, set = Self::set_playing)]
        playing: Cell<bool>,
        /// How far the track is, 0 to 1: where the arm lies on the record.
        #[property(get, set = Self::set_progress)]
        progress: Cell<f64>,
        /// Nothing moves (the "Reduce effects" setting).
        #[property(get, set = Self::set_still)]
        still: Cell<bool>,

        /// The label's turn, in degrees, and its speed, in degrees per second.
        angle: Cell<f64>,
        speed: Cell<f64>,
        /// 0: the arm at rest; 1: on the record.
        arm: Cell<f64>,
        /// 0 to 1: the record dropping in.
        dropped: Cell<f64>,
        ticking: Cell<bool>,
        last_frame: Cell<Option<i64>>,
    }

    impl Turntable {
        fn set_artwork(&self, artwork: Option<gdk::Paintable>) {
            self.artwork.replace(artwork);
            self.obj().queue_draw();
        }

        fn set_playing(&self, playing: bool) {
            if self.playing.replace(playing) != playing {
                self.animate();
            }
        }

        fn set_progress(&self, progress: f64) {
            let progress = progress.clamp(0.0, 1.0);
            // (Only a visible step of the arm is worth a frame.)
            if (self.progress.get() - progress).abs() > 0.002 {
                self.progress.set(progress);
                if !self.ticking.get() {
                    self.obj().queue_draw();
                }
            }
        }

        fn set_still(&self, still: bool) {
            if self.still.replace(still) != still {
                self.animate();
            }
        }

        fn settled(&self) -> bool {
            let target = if self.playing.get() { 1.0 } else { 0.0 };
            !self.playing.get()
                && self.speed.get().abs() < 0.5
                && (self.arm.get() - target).abs() < 0.002
                && self.dropped.get() >= 1.0
        }

        /// Go to where `playing` says: at once when `still`, else frame by
        /// frame until it is reached (or for as long as the record turns).
        pub(super) fn animate(&self) {
            let obj = self.obj();
            if self.still.get() {
                self.speed.set(0.0);
                self.arm.set(if self.playing.get() { 1.0 } else { 0.0 });
                self.dropped.set(1.0);
                obj.queue_draw();
                return;
            }
            if self.ticking.get() || self.settled() {
                return;
            }
            self.ticking.set(true);
            self.last_frame.set(None);
            obj.add_tick_callback(|obj, clock| {
                let imp = obj.imp();
                let now = clock.frame_time();
                let dt = imp
                    .last_frame
                    .replace(Some(now))
                    .map_or(0.0, |last| ((now - last) as f64 / 1e6).clamp(0.0, 0.05));
                if imp.still.get() {
                    imp.ticking.set(false);
                    return glib::ControlFlow::Break;
                }
                let playing = imp.playing.get();
                let ease =
                    |from: f64, to: f64, over: f64| from + (to - from) * (1.0 - (-dt / over).exp());
                let speed = ease(imp.speed.get(), if playing { SPIN } else { 0.0 }, SPIN_EASE);
                imp.speed.set(speed);
                imp.angle.set((imp.angle.get() + speed * dt) % 360.0);
                imp.arm.set(ease(
                    imp.arm.get(),
                    if playing { 1.0 } else { 0.0 },
                    ARM_EASE,
                ));
                imp.dropped.set((imp.dropped.get() + dt / DROP).min(1.0));
                obj.queue_draw();
                if imp.settled() {
                    imp.speed.set(0.0);
                    imp.ticking.set(false);
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            });
        }

        fn draw_arm(&self, snapshot: &gtk::Snapshot, s: f32, shadow: bool) {
            let on = ARM_FIRST + (ARM_LAST - ARM_FIRST) * self.progress.get();
            let angle = ARM_REST + (on - ARM_REST) * self.arm.get();
            let tint = |color: gdk::RGBA| if shadow { rgba(0, 0, 0, 0.30) } else { color };
            let rect =
                |x: f32, y: f32, w: f32, h: f32| graphene::Rect::new(x * s, y * s, w * s, h * s);
            snapshot.save();
            let offset = if shadow { s * 0.012 } else { 0.0 };
            snapshot.translate(&graphene::Point::new(
                PIVOT_X * s + offset,
                PIVOT_Y * s + offset * 1.5,
            ));
            snapshot.rotate(angle as f32);
            // The counterweight, the tube, the headshell and its finger lift.
            fill_rounded(
                snapshot,
                rect(-0.036, -0.13, 0.072, 0.078),
                0.012 * s,
                &tint(rgba(0x3b, 0x40, 0x4b, 1.0)),
            );
            let tube = rect(-0.010, -0.06, 0.020, 0.49);
            if shadow {
                snapshot.append_color(&tint(rgba(0, 0, 0, 1.0)), &tube);
            } else {
                snapshot.append_linear_gradient(
                    &tube,
                    &graphene::Point::new(tube.x(), 0.0),
                    &graphene::Point::new(tube.x() + tube.width(), 0.0),
                    &[
                        gsk::ColorStop::new(0.0, rgba(0xe6, 0xe8, 0xee, 1.0)),
                        gsk::ColorStop::new(1.0, rgba(0x8b, 0x90, 0x9b, 1.0)),
                    ],
                );
            }
            snapshot.append_color(
                &tint(rgba(0xc4, 0xc8, 0xd0, 1.0)),
                &rect(0.028, 0.452, 0.034, 0.011),
            );
            fill_rounded(
                snapshot,
                rect(-0.03, 0.415, 0.06, 0.10),
                0.010 * s,
                &tint(rgba(0xff, 0xb4, 0x54, 1.0)),
            );
            snapshot.restore();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Turntable {
        const NAME: &'static str = "TFTurntable";
        type Type = super::Turntable;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("turntable");
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for Turntable {}

    impl WidgetImpl for Turntable {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (64, 320, -1, -1)
        }

        fn map(&self) {
            self.parent_map();
            // The record drops in, and turns if it should.
            self.animate();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            let s = w.min(h);
            if s < 16.0 {
                return;
            }
            let point = graphene::Point::new;
            snapshot.save();
            snapshot.translate(&point((w - s) / 2.0, (h - s) / 2.0));

            // The plinth.
            let whole = graphene::Rect::new(0.0, 0.0, s, s);
            let plinth = gsk::RoundedRect::from_rect(whole, s * 0.06);
            snapshot.push_rounded_clip(&plinth);
            snapshot.append_linear_gradient(
                &whole,
                &point(0.0, 0.0),
                &point(0.0, s),
                &[
                    gsk::ColorStop::new(0.0, rgba(0x2a, 0x2e, 0x38, 0.86)),
                    gsk::ColorStop::new(1.0, rgba(0x14, 0x16, 0x1c, 0.90)),
                ],
            );
            snapshot.pop();
            snapshot.append_border(&plinth, &[1.0; 4], &[rgba(255, 255, 255, 0.08); 4]);

            // The platter.
            let (cx, cy, r) = (RECORD_X * s, RECORD_Y * s, RECORD_R * s);
            let platter = disc(cx, cy, PLATTER_R * s);
            snapshot.append_outset_shadow(
                &platter,
                &rgba(0, 0, 0, 0.55),
                0.0,
                s * 0.012,
                0.0,
                s * 0.05,
            );
            fill_disc(
                snapshot,
                cx,
                cy,
                PLATTER_R * s,
                &rgba(0x33, 0x38, 0x43, 1.0),
            );

            // The record: it drops in, a little large and faint at first.
            let dropped = self.dropped.get() as f32;
            let eased = 1.0 - (1.0 - dropped).powi(3);
            snapshot.save();
            snapshot.translate(&point(cx, cy));
            if eased < 1.0 {
                let scale = 1.08 - 0.08 * eased;
                snapshot.scale(scale, scale);
                snapshot.push_opacity(eased as f64);
            }
            let vinyl = disc(0.0, 0.0, r);
            let bounds = *vinyl.bounds();
            snapshot.push_rounded_clip(&vinyl);
            snapshot.append_color(&rgba(0x08, 0x08, 0x0a, 1.0), &bounds);
            // Grooves, then the light lying across them (it does not turn).
            let period = (s * 0.009).max(2.5) / r;
            snapshot.append_repeating_radial_gradient(
                &bounds,
                &point(0.0, 0.0),
                r,
                r,
                0.0,
                period,
                &[
                    gsk::ColorStop::new(0.0, rgba(0x0a, 0x0a, 0x0c, 1.0)),
                    gsk::ColorStop::new(0.5, rgba(0x1d, 0x1e, 0x24, 1.0)),
                    gsk::ColorStop::new(1.0, rgba(0x0a, 0x0a, 0x0c, 1.0)),
                ],
            );
            for gap in [0.62, 0.78, 0.9] {
                ring(
                    snapshot,
                    0.0,
                    0.0,
                    r * gap,
                    (s * 0.004).max(1.0),
                    rgba(0, 0, 0, 0.55),
                );
            }
            fill_disc(
                snapshot,
                0.0,
                0.0,
                r * (LABEL + 0.06),
                &rgba(0x0c, 0x0c, 0x0f, 1.0),
            );
            let light = |at: f32, alpha: f32| gsk::ColorStop::new(at, rgba(255, 255, 255, alpha));
            snapshot.append_conic_gradient(
                &bounds,
                &point(0.0, 0.0),
                28.0,
                &[
                    light(0.0, 0.0),
                    light(0.06, 0.0),
                    light(0.125, 0.16),
                    light(0.19, 0.0),
                    light(0.56, 0.0),
                    light(0.625, 0.12),
                    light(0.69, 0.0),
                    light(1.0, 0.0),
                ],
            );
            snapshot.pop();
            ring(snapshot, 0.0, 0.0, r, 1.0, rgba(255, 255, 255, 0.06));

            // The label: the artwork, turning.
            let label_r = r * LABEL;
            let label = disc(0.0, 0.0, label_r);
            snapshot.save();
            snapshot.rotate(self.angle.get() as f32);
            snapshot.push_rounded_clip(&label);
            snapshot.append_color(&rgba(0xff, 0xb4, 0x54, 1.0), label.bounds());
            if let Some(artwork) = self.artwork.borrow().as_ref() {
                let (iw, ih) = (
                    artwork.intrinsic_width().max(1) as f32,
                    artwork.intrinsic_height().max(1) as f32,
                );
                let scale = 2.0 * label_r / iw.min(ih);
                let (dw, dh) = (iw * scale, ih * scale);
                let place = graphene::Rect::new(-dw / 2.0, -dh / 2.0, dw, dh);
                match artwork.downcast_ref::<gdk::Texture>() {
                    Some(texture) => snapshot.append_scaled_texture(
                        texture,
                        gsk::ScalingFilter::Trilinear,
                        &place,
                    ),
                    None => {
                        snapshot.translate(&point(place.x(), place.y()));
                        artwork.snapshot(snapshot, dw as f64, dh as f64);
                    }
                }
            }
            snapshot.pop();
            snapshot.restore();
            ring(
                snapshot,
                0.0,
                0.0,
                label_r,
                (s * 0.004).max(1.0),
                rgba(0, 0, 0, 0.45),
            );
            // The spindle.
            fill_disc(snapshot, 0.0, 0.0, s * 0.022, &rgba(0x10, 0x10, 0x13, 1.0));
            fill_disc(snapshot, 0.0, 0.0, s * 0.011, &rgba(0xd8, 0xda, 0xe0, 1.0));
            if eased < 1.0 {
                snapshot.pop();
            }
            snapshot.restore();

            // The tonearm on its pivot.
            let (px, py) = (PIVOT_X * s, PIVOT_Y * s);
            fill_disc(snapshot, px, py, s * 0.062, &rgba(0x1c, 0x1f, 0x26, 1.0));
            ring(snapshot, px, py, s * 0.062, 1.0, rgba(255, 255, 255, 0.10));
            self.draw_arm(snapshot, s, true);
            self.draw_arm(snapshot, s, false);
            fill_disc(snapshot, px, py, s * 0.034, &rgba(0x4a, 0x50, 0x5c, 1.0));
            fill_disc(snapshot, px, py, s * 0.014, &rgba(0xd8, 0xda, 0xe0, 1.0));

            snapshot.restore();
        }
    }
}
