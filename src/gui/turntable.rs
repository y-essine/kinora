// SPDX-License-Identifier: GPL-3.0-or-later

//! Kinora: the DJ's record player (see `docs/design.md`, "DJ").
//!
//! A square deck: a platter, a record whose label is the track's artwork, and
//! a tonearm. While `playing`, the label turns and the arm lies on the record,
//! moving inwards with `progress`; paused, the record eases to a stop and the
//! arm goes back to its rest. A new deck's record drops in, so a change of
//! track reads as a change of record.
//!
//! The record can be taken by hand: pressed and dragged around its centre,
//! it turns with the pointer, and the deck says so ([`Hand`]) for whoever
//! plays the sound to follow (the page scratches, see `gui/scratch.rs`). A
//! press without a drag is a tap.
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
pub const SPIN: f64 = 72.0;
/// The pointer has to move this far for a press to be a drag, in pixels.
const DRAG_FROM: f64 = 4.0;
/// Nearer the spindle than this (in parts of the record's radius), the
/// pointer's angle says little: the record keeps its turn.
const HUB: f64 = 0.1;

/// What a hand does with the deck.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hand {
    /// The pointer went down on the record. Answer `false` and the record
    /// is not for taking now.
    Down,
    /// The record is dragged: so many degrees since it was taken (clockwise,
    /// the way it plays, is positive).
    Turn(f64),
    /// The record is let go after a drag. It stays [held](Turntable::set_held)
    /// until the handler says otherwise.
    Up,
    /// A press and release without a move, anywhere on the deck.
    Tap,
    /// A press on the record came to nothing (it was taken away).
    Away,
}

/// A press that is under way.
#[derive(Clone, Copy)]
struct Press {
    start: (f64, f64),
    /// The record was pressed, and may be taken.
    on_record: bool,
    dragged: bool,
    /// The pointer's angle around the spindle when last looked at, in radians.
    pointer: f64,
    /// Degrees turned since the press, and the label's turn at the press.
    turned: f64,
    from: f64,
}
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

    /// Whether a point of the widget is on the record.
    pub fn on_record(&self, x: f64, y: f64) -> bool {
        let (cx, cy, r) = self.record();
        (x - cx).hypot(y - cy) <= r
    }

    /// Be told what a hand does with the deck (see [`Hand`]). The answer
    /// only matters for [`Hand::Down`].
    pub fn connect_hand(&self, handler: impl Fn(&Self, Hand) -> bool + 'static) {
        self.imp().handler.replace(Some(Box::new(handler)));
    }

    /// The record is in someone's hand: it does not turn by itself, and the
    /// arm stays on it.
    pub fn is_held(&self) -> bool {
        self.imp().held.get()
    }

    /// Let the record go (or take it): let go while `playing`, it turns on at
    /// once, without running up again.
    pub fn set_held(&self, held: bool) {
        let imp = self.imp();
        if imp.held.replace(held) != held {
            if !held && self.playing() && !self.still() {
                imp.speed.set(SPIN);
            }
            imp.animate();
            self.queue_draw();
        }
    }

    /// The label's turn, in degrees.
    pub fn turn(&self) -> f64 {
        self.imp().angle.get()
    }

    /// Turn the label to `degrees` (for whoever holds the record).
    pub fn set_turn(&self, degrees: f64) {
        self.imp().angle.set(degrees.rem_euclid(360.0));
        self.queue_draw();
    }

    fn emit(&self, hand: Hand) -> bool {
        let handler = self.imp().handler.borrow();
        handler.as_ref().is_some_and(|handler| handler(self, hand))
    }

    /// The pointer went down at (`x`, `y`).
    fn press(&self, x: f64, y: f64) {
        let (cx, cy, _) = self.record();
        let on_record = self.on_record(x, y) && self.emit(Hand::Down);
        self.imp().press.set(Some(Press {
            start: (x, y),
            on_record,
            dragged: false,
            pointer: (y - cy).atan2(x - cx),
            turned: 0.0,
            from: self.turn(),
        }));
    }

    /// The pressed pointer is now at (`x`, `y`). `true`: the record is being
    /// dragged.
    fn drag(&self, x: f64, y: f64) -> bool {
        let imp = self.imp();
        let Some(mut press) = imp.press.get() else {
            return false;
        };
        let (cx, cy, r) = self.record();
        if !press.dragged && (x - press.start.0).hypot(y - press.start.1) > DRAG_FROM {
            press.dragged = true;
            if press.on_record {
                // Taken from here: not a jump back to where it was pressed.
                press.pointer = (y - cy).atan2(x - cx);
                press.from = self.turn();
                self.set_held(true);
            }
        }
        if press.dragged && press.on_record {
            if (x - cx).hypot(y - cy) > r * HUB {
                let pointer = (y - cy).atan2(x - cx);
                // The short way round from where it was.
                let step = (pointer - press.pointer + std::f64::consts::PI)
                    .rem_euclid(std::f64::consts::TAU)
                    - std::f64::consts::PI;
                press.pointer = pointer;
                press.turned += step.to_degrees();
            }
            self.set_turn(press.from + press.turned);
            self.emit(Hand::Turn(press.turned));
        }
        imp.press.set(Some(press));
        press.dragged && press.on_record
    }

    /// The pointer came up (`cancelled`: was taken away).
    fn lift(&self, cancelled: bool) {
        let Some(press) = self.imp().press.take() else {
            return;
        };
        if press.dragged && press.on_record {
            self.emit(Hand::Up);
        } else if !press.dragged && !cancelled {
            self.emit(Hand::Tap);
        } else if press.on_record {
            self.emit(Hand::Away);
        }
    }

    /// Test hook: take the record and turn it as a hand would, through the
    /// same steps as a drag: `turns` is a list of (milliseconds since the
    /// press, degrees turned by then); in between, the hand moves evenly.
    /// After the last one the record is let go.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_scratch(&self, turns: Vec<(u64, f64)>) {
        let (cx, cy, r) = self.record();
        // Pressed at three o'clock, two thirds out.
        let radius = r * 0.66;
        self.press(cx + radius, cy);
        if !self.imp().press.get().is_some_and(|press| press.on_record) {
            log::info!("DEV: scratch: the record is not for taking");
            self.imp().press.take();
            return;
        }
        let began = std::time::Instant::now();
        let deck = self.downgrade();
        glib::timeout_add_local(std::time::Duration::from_millis(8), move || {
            let Some(deck) = deck.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let now = began.elapsed().as_millis() as u64;
            let last = turns.last().copied().unwrap_or((0, 0.0));
            let at = turns.partition_point(|(t, _)| *t <= now);
            let degrees = match (at.checked_sub(1).map(|i| turns[i]), turns.get(at)) {
                (Some((t0, d0)), Some(&(t1, d1))) => {
                    d0 + (d1 - d0) * (now - t0) as f64 / (t1 - t0).max(1) as f64
                }
                (None, Some(&(t1, d1))) => d1 * now as f64 / t1.max(1) as f64,
                _ => last.1,
            };
            let angle = degrees.to_radians();
            deck.drag(cx + radius * angle.cos(), cy + radius * angle.sin());
            if now >= last.0 {
                deck.lift(false);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
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
        pub(super) angle: Cell<f64>,
        pub(super) speed: Cell<f64>,
        /// 0: the arm at rest; 1: on the record.
        arm: Cell<f64>,
        /// 0 to 1: the record dropping in.
        dropped: Cell<f64>,
        ticking: Cell<bool>,
        last_frame: Cell<Option<i64>>,

        /// The record is in a hand (see [`super::Turntable::set_held`]).
        pub(super) held: Cell<bool>,
        pub(super) press: Cell<Option<Press>>,
        #[allow(clippy::type_complexity)]
        pub(super) handler: RefCell<Option<Box<dyn Fn(&super::Turntable, Hand) -> bool>>>,
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

        /// The arm lies on the record: it plays, or a hand has it.
        fn arm_on(&self) -> bool {
            self.playing.get() || self.held.get()
        }

        fn settled(&self) -> bool {
            let target = if self.arm_on() { 1.0 } else { 0.0 };
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
                self.arm.set(if self.arm_on() { 1.0 } else { 0.0 });
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
                // In a hand, the record turns with it and not by itself.
                if imp.held.get() {
                    imp.speed.set(0.0);
                } else {
                    let speed = ease(imp.speed.get(), if playing { SPIN } else { 0.0 }, SPIN_EASE);
                    imp.speed.set(speed);
                    imp.angle.set((imp.angle.get() + speed * dt) % 360.0);
                }
                imp.arm.set(ease(
                    imp.arm.get(),
                    if imp.arm_on() { 1.0 } else { 0.0 },
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
    impl ObjectImpl for Turntable {
        fn constructed(&self) {
            self.parent_constructed();
            // A press on the deck: a tap, or the record taken by hand.
            let drag = gtk::GestureDrag::new();
            drag.connect_drag_begin(|gesture, x, y| {
                if let Some(deck) = gesture.widget().and_downcast::<super::Turntable>() {
                    deck.press(x, y);
                }
            });
            drag.connect_drag_update(|gesture, dx, dy| {
                let (Some(deck), Some((x, y))) = (
                    gesture.widget().and_downcast::<super::Turntable>(),
                    gesture.start_point(),
                ) else {
                    return;
                };
                // The record is ours: nothing else is to drag with it.
                if deck.drag(x + dx, y + dy) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            });
            drag.connect_drag_end(|gesture, _, _| {
                if let Some(deck) = gesture.widget().and_downcast::<super::Turntable>() {
                    deck.lift(false);
                }
            });
            drag.connect_cancel(|gesture, _| {
                if let Some(deck) = gesture.widget().and_downcast::<super::Turntable>() {
                    deck.lift(true);
                }
            });
            self.obj().add_controller(drag);
        }
    }

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
