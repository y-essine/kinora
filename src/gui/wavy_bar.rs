// SPDX-License-Identifier: GPL-3.0-or-later

//! Kinora: the line of a track, in the manner of Material 3's wavy progress
//! (see `docs/design.md`, "DJ").
//!
//! The played part is a sine wave, the rest a flat track that ends in a stop
//! dot; in between stands a thin handle. While `playing` the wave drifts
//! towards the handle; paused, it flattens. A press or a drag anywhere on the
//! bar moves the handle and tells whoever listens ([`WavyBar::connect_seek`]);
//! the arrow keys step it. `buffered` lights the part of the track that is
//! already loaded.
//!
//! One stroked path and three small fills. Frames are asked for only while
//! something moves and the bar is on screen, and the wave is redrawn only
//! when it has moved half a pixel. With `still` (the "Reduce effects"
//! setting) or the desktop's animations off, nothing drifts: the wave simply
//! stands while playing and lies flat while paused.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

glib::wrapper! {
    pub struct WavyBar(ObjectSubclass<imp::WavyBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// Room either side of the line, so the handle never leaves the widget.
const PAD: f32 = 6.0;
const HEIGHT: i32 = 28;
/// The line's thickness, the wave's height either side of the middle and its
/// length, in pixels.
const LINE: f32 = 4.0;
const AMPLITUDE: f32 = 3.0;
const WAVELENGTH: f32 = 40.0;
/// How fast the wave drifts while playing, in pixels per second.
const DRIFT: f64 = 16.0;
/// The empty space either side of the handle.
const GAP: f32 = 6.0;
const HANDLE_HEIGHT: f32 = 22.0;
/// The handle's width, at rest and in a hand.
const HANDLE: f32 = 4.0;
const HANDLE_HELD: f32 = 2.0;
/// How quickly the wave rises and flattens, and the handle thins, in seconds.
const WAVE_EASE: f64 = 0.14;
const HANDLE_EASE: f64 = 0.05;
/// A step of the arrow keys, in parts of the track.
const STEP: f64 = 0.02;

impl WavyBar {
    /// Be told where the track was asked to go (0 to 1), for every step of a
    /// drag and every key.
    pub fn connect_seek(&self, handler: impl Fn(&Self, f64) + 'static) {
        self.imp().handler.replace(Some(Box::new(handler)));
    }

    /// The pointer at `x` is this far along the track.
    fn fraction_at(&self, x: f64) -> f64 {
        let span = (self.width() as f64 - 2.0 * PAD as f64).max(1.0);
        ((x - PAD as f64) / span).clamp(0.0, 1.0)
    }

    /// The user moved the handle.
    fn seek(&self, fraction: f64) {
        let imp = self.imp();
        imp.value.set(fraction.clamp(0.0, 1.0));
        imp.announce();
        self.queue_draw();
        let handler = imp.handler.borrow();
        if let Some(handler) = handler.as_ref() {
            handler(self, imp.value.get());
        }
    }
}

fn pill(snapshot: &gtk::Snapshot, x: f32, y: f32, w: f32, h: f32, color: &gdk::RGBA) {
    let rect = graphene::Rect::new(x, y, w, h);
    snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(rect, w.min(h) / 2.0));
    snapshot.append_color(color, &rect);
    snapshot.pop();
}

mod imp {
    use std::cell::{Cell, RefCell};

    use super::*;

    #[derive(glib::Properties, Default)]
    #[properties(wrapper_type = super::WavyBar)]
    pub struct WavyBar {
        /// How far the track is, 0 to 1. Not taken while the bar is dragged.
        #[property(get, set = Self::set_value)]
        pub(super) value: Cell<f64>,
        /// Up to where the track is loaded, 0 to 1 (0: not known).
        #[property(get, set = Self::set_buffered)]
        buffered: Cell<f64>,
        /// The wave drifts.
        #[property(get, set = Self::set_playing)]
        playing: Cell<bool>,
        /// Nothing moves by itself (the "Reduce effects" setting).
        #[property(get, set = Self::set_still)]
        still: Cell<bool>,

        /// 0: flat; 1: the full wave.
        wave: Cell<f64>,
        /// How far the wave has drifted, in pixels, and where it was drawn.
        shift: Cell<f64>,
        drawn_shift: Cell<f64>,
        /// 0: the handle at rest; 1: in a hand.
        grip: Cell<f64>,
        dragging: Cell<bool>,
        tick: RefCell<Option<gtk::TickCallbackId>>,
        last_frame: Cell<Option<i64>>,
        /// The value assistive technology was last told.
        announced: Cell<f64>,
        #[allow(clippy::type_complexity)]
        pub(super) handler: RefCell<Option<Box<dyn Fn(&super::WavyBar, f64)>>>,
    }

    impl WavyBar {
        fn set_value(&self, value: f64) {
            if self.dragging.get() {
                return;
            }
            let value = if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let old = self.value.replace(value);
            // (Only a visible step is worth a frame.)
            let obj = self.obj();
            if (old - value).abs() * obj.width() as f64 >= 0.25 {
                obj.queue_draw();
            }
            if (self.announced.get() - value).abs() >= 0.01 {
                self.announce();
            }
        }

        fn set_buffered(&self, buffered: f64) {
            let buffered = if buffered.is_finite() {
                buffered.clamp(0.0, 1.0)
            } else {
                0.0
            };
            if (self.buffered.replace(buffered) - buffered).abs() > 0.001 {
                self.obj().queue_draw();
            }
        }

        fn set_playing(&self, playing: bool) {
            if self.playing.replace(playing) != playing {
                self.animate();
            }
        }

        fn set_still(&self, still: bool) {
            if self.still.replace(still) != still {
                self.animate();
            }
        }

        pub(super) fn announce(&self) {
            let value = self.value.get();
            self.announced.set(value);
            self.obj()
                .update_property(&[gtk::accessible::Property::ValueNow(value)]);
        }

        /// Whether anything may move by itself.
        fn moves(&self) -> bool {
            !self.still.get()
                && gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
        }

        fn wave_target(&self) -> f64 {
            if self.playing.get() { 1.0 } else { 0.0 }
        }

        fn grip_target(&self) -> f64 {
            if self.dragging.get() { 1.0 } else { 0.0 }
        }

        fn settled(&self) -> bool {
            !self.playing.get()
                && (self.wave.get() - self.wave_target()).abs() < 0.01
                && (self.grip.get() - self.grip_target()).abs() < 0.01
        }

        fn stop(&self) {
            if let Some(tick) = self.tick.take() {
                tick.remove();
            }
        }

        /// Go to where `playing` says: at once when nothing may move, else
        /// frame by frame until it is reached (or for as long as it plays).
        pub(super) fn animate(&self) {
            let obj = self.obj();
            if !obj.is_mapped() || !self.moves() {
                self.stop();
                self.wave.set(self.wave_target());
                self.grip.set(self.grip_target());
                obj.queue_draw();
                return;
            }
            if self.tick.borrow().is_some() || self.settled() {
                return;
            }
            self.last_frame.set(None);
            let tick = obj.add_tick_callback(|obj, clock| {
                let imp = obj.imp();
                let now = clock.frame_time();
                let dt = imp
                    .last_frame
                    .replace(Some(now))
                    .map_or(0.0, |last| ((now - last) as f64 / 1e6).clamp(0.0, 0.05));
                let done = |imp: &Self| {
                    // (Returning Break removes the callback: only forget it.)
                    drop(imp.tick.take());
                    imp.wave.set(imp.wave_target());
                    imp.grip.set(imp.grip_target());
                    imp.obj().queue_draw();
                    glib::ControlFlow::Break
                };
                if !imp.moves() {
                    return done(imp);
                }
                let ease =
                    |from: f64, to: f64, over: f64| from + (to - from) * (1.0 - (-dt / over).exp());
                let wave = ease(imp.wave.get(), imp.wave_target(), WAVE_EASE);
                let grip = ease(imp.grip.get(), imp.grip_target(), HANDLE_EASE);
                let shaping = (wave - imp.wave.get()).abs() > 0.0005
                    || (grip - imp.grip.get()).abs() > 0.0005;
                imp.wave.set(wave);
                imp.grip.set(grip);
                // (The drift slows with the wave, so it comes to rest flat.)
                imp.shift
                    .set((imp.shift.get() + DRIFT * dt * wave) % WAVELENGTH as f64);
                let moved = (imp.shift.get() - imp.drawn_shift.get()).abs();
                if shaping || moved >= 0.5 {
                    obj.queue_draw();
                }
                if imp.settled() {
                    return done(imp);
                }
                glib::ControlFlow::Continue
            });
            self.tick.replace(Some(tick));
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WavyBar {
        const NAME: &'static str = "TFWavyBar";
        type Type = super::WavyBar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("wavybar");
            klass.set_accessible_role(gtk::AccessibleRole::Slider);
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for WavyBar {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_cursor_from_name(Some("pointer"));
            obj.update_property(&[
                gtk::accessible::Property::ValueMin(0.0),
                gtk::accessible::Property::ValueMax(1.0),
                gtk::accessible::Property::ValueNow(0.0),
            ]);

            // A press anywhere on the bar takes the handle there; a drag
            // carries it along. (Claimed: nothing else is to drag with it.)
            let drag = gtk::GestureDrag::new();
            drag.connect_drag_begin(|gesture, x, _| {
                let Some(bar) = gesture.widget().and_downcast::<super::WavyBar>() else {
                    return;
                };
                gesture.set_state(gtk::EventSequenceState::Claimed);
                bar.imp().dragging.set(true);
                bar.imp().animate();
                bar.seek(bar.fraction_at(x));
            });
            drag.connect_drag_update(|gesture, dx, _| {
                let (Some(bar), Some((x, _))) = (
                    gesture.widget().and_downcast::<super::WavyBar>(),
                    gesture.start_point(),
                ) else {
                    return;
                };
                if bar.imp().dragging.get() {
                    bar.seek(bar.fraction_at(x + dx));
                }
            });
            let let_go = |gesture: &gtk::GestureDrag| {
                if let Some(bar) = gesture.widget().and_downcast::<super::WavyBar>() {
                    bar.imp().dragging.set(false);
                    bar.imp().animate();
                }
            };
            drag.connect_drag_end(move |gesture, _, _| let_go(gesture));
            drag.connect_cancel(move |gesture, _| let_go(gesture));
            obj.add_controller(drag);

            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(|keys, key, _, modifiers| {
                let Some(bar) = keys.widget().and_downcast::<super::WavyBar>() else {
                    return glib::Propagation::Proceed;
                };
                if !(modifiers & gtk::accelerator_get_default_mod_mask()).is_empty() {
                    return glib::Propagation::Proceed;
                }
                let back = if bar.direction() == gtk::TextDirection::Rtl {
                    1.0
                } else {
                    -1.0
                };
                let to = match key {
                    gdk::Key::Left | gdk::Key::KP_Left => bar.value() + back * STEP,
                    gdk::Key::Right | gdk::Key::KP_Right => bar.value() - back * STEP,
                    gdk::Key::Home | gdk::Key::KP_Home => 0.0,
                    _ => return glib::Propagation::Proceed,
                };
                bar.seek(to);
                glib::Propagation::Stop
            });
            obj.add_controller(keys);
        }

        fn dispose(&self) {
            self.stop();
        }
    }

    impl WidgetImpl for WavyBar {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (80, 240, -1, -1),
                _ => (HEIGHT, HEIGHT, -1, -1),
            }
        }

        fn map(&self) {
            self.parent_map();
            self.animate();
        }

        fn unmap(&self) {
            // No frames while the bar is not on screen.
            self.stop();
            self.parent_unmap();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            if w < 2.0 * PAD + 4.0 * LINE {
                return;
            }
            let accent = obj.color();
            let white = |alpha: f32| gdk::RGBA::new(1.0, 1.0, 1.0, alpha);
            let rtl = obj.direction() == gtk::TextDirection::Rtl;
            if rtl {
                snapshot.save();
                snapshot.translate(&graphene::Point::new(w, 0.0));
                snapshot.scale(-1.0, 1.0);
            }
            let (x0, x1, cy) = (PAD, w - PAD, h / 2.0);
            let at = |fraction: f64| x0 + fraction as f32 * (x1 - x0);
            let hx = at(self.value.get());

            // The played part: a wave from the start to just before the handle.
            let (from, to) = (x0 + LINE / 2.0, hx - GAP - LINE / 2.0);
            if to > from {
                let amplitude = AMPLITUDE * self.wave.get() as f32;
                let shift = self.shift.get() as f32;
                self.drawn_shift.set(self.shift.get());
                let y = |x: f32| {
                    cy + amplitude * (std::f32::consts::TAU * (x - x0 - shift) / WAVELENGTH).sin()
                };
                let path = gsk::PathBuilder::new();
                path.move_to(from, y(from));
                if amplitude > 0.02 {
                    let mut x = from;
                    while x < to {
                        x = (x + 2.0).min(to);
                        path.line_to(x, y(x));
                    }
                } else {
                    path.line_to(to, cy);
                }
                let stroke = gsk::Stroke::new(LINE);
                stroke.set_line_cap(gsk::LineCap::Round);
                stroke.set_line_join(gsk::LineJoin::Round);
                snapshot.append_stroke(&path.to_path(), &stroke, &accent);
            }

            // The rest: a flat track, lighter as far as it is loaded, and the
            // stop dot at its end.
            let (from, to) = (hx + GAP, x1 - 2.0 * LINE);
            if to - from >= LINE {
                pill(
                    snapshot,
                    from,
                    cy - LINE / 2.0,
                    to - from,
                    LINE,
                    &white(0.14),
                );
                let loaded = at(self.buffered.get()).min(to);
                if loaded - from >= LINE {
                    pill(
                        snapshot,
                        from,
                        cy - LINE / 2.0,
                        loaded - from,
                        LINE,
                        &white(0.30),
                    );
                }
            }
            pill(snapshot, x1 - LINE, cy - LINE / 2.0, LINE, LINE, &accent);

            // The handle: thinner in a hand.
            let width = HANDLE + (HANDLE_HELD - HANDLE) * self.grip.get() as f32;
            pill(
                snapshot,
                hx - width / 2.0,
                cy - HANDLE_HEIGHT / 2.0,
                width,
                HANDLE_HEIGHT,
                &accent,
            );
            if rtl {
                snapshot.restore();
            }
        }
    }
}
