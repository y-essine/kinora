// SPDX-License-Identifier: GPL-3.0-or-later

//! Kinora: a button that answers a press with a ripple, as Material's do (see
//! `docs/design.md`, "DJ").
//!
//! An ordinary `GtkButton` in every other way: its look, its shape (and the
//! morphing of its corners) are CSS. The ripple spreads from where the button
//! was pressed (from the middle for a key), in the button's text colour, under
//! the icon and inside the button's rounded shape; it fades once the press is
//! over. Frames are asked for only while a ripple lives. With "Reduce effects"
//! or the desktop's animations off there is none: the pressed colour says it.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

glib::wrapper! {
    pub struct RippleButton(ObjectSubclass<imp::RippleButton>)
        @extends gtk::Button, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

/// How long the ripple takes to cover the button, how long it stays at least,
/// and how long it takes to fade, in seconds.
const SPREAD: f64 = 0.45;
const HOLD: f64 = 0.18;
const FADE: f64 = 0.30;
/// The ripple at its strongest, as the opacity of the text colour.
const STRENGTH: f32 = 0.20;

/// A ripple that is under way (times as the frame clock tells them, in µs).
#[derive(Clone, Copy)]
struct Ripple {
    origin: (f32, f32),
    began: i64,
    released: Option<i64>,
}

impl RippleButton {
    fn press(&self, x: f64, y: f64) {
        let imp = self.imp();
        let still = self
            .root()
            .is_some_and(|r| r.has_css_class("reduce-effects"))
            || !gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations());
        if still || !self.is_mapped() {
            return;
        }
        imp.ripple.set(Some(Ripple {
            origin: (x as f32, y as f32),
            began: glib::monotonic_time(),
            released: None,
        }));
        imp.now.set(glib::monotonic_time());
        if imp.ticking.replace(true) {
            return;
        }
        self.add_tick_callback(|button, clock| {
            let imp = button.imp();
            imp.now.set(clock.frame_time());
            button.queue_draw();
            let over = imp
                .ripple
                .get()
                .is_none_or(|r| r.fade(imp.now.get()) <= 0.0);
            if over || !button.is_mapped() {
                imp.ripple.set(None);
                imp.ticking.set(false);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    fn release(&self) {
        let imp = self.imp();
        if let Some(mut ripple) = imp.ripple.get() {
            if ripple.released.is_none() {
                ripple.released = Some(glib::monotonic_time());
                imp.ripple.set(Some(ripple));
            }
        }
    }
}

impl Ripple {
    fn since(from: i64, now: i64) -> f64 {
        (now - from).max(0) as f64 / 1e6
    }

    /// 0 to 1: how far it has spread.
    fn spread(&self, now: i64) -> f32 {
        let t = (Self::since(self.began, now) / SPREAD).min(1.0);
        (1.0 - (1.0 - t).powi(3)) as f32
    }

    /// 1 to 0: how much of it is left.
    fn fade(&self, now: i64) -> f32 {
        let Some(released) = self.released else {
            return 1.0;
        };
        let from = released.max(self.began + (HOLD * 1e6) as i64);
        (1.0 - Self::since(from, now) / FADE).clamp(0.0, 1.0) as f32
    }
}

mod imp {
    use std::cell::Cell;

    use super::*;

    #[derive(Default)]
    pub struct RippleButton {
        pub(super) ripple: Cell<Option<Ripple>>,
        pub(super) now: Cell<i64>,
        pub(super) ticking: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RippleButton {
        const NAME: &'static str = "TFRippleButton";
        type Type = super::RippleButton;
        type ParentType = gtk::Button;
    }

    impl ObjectImpl for RippleButton {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            // (Clips what is drawn below to the button's rounded shape.)
            obj.set_overflow(gtk::Overflow::Hidden);

            // Watch the press without taking it: the button's own gesture
            // still clicks.
            let click = gtk::GestureClick::new();
            click.set_button(gdk::BUTTON_PRIMARY);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            click.connect_pressed(|gesture, _, x, y| {
                if let Some(button) = gesture.widget().and_downcast::<super::RippleButton>() {
                    button.press(x, y);
                }
            });
            let release = |gesture: &gtk::GestureClick| {
                if let Some(button) = gesture.widget().and_downcast::<super::RippleButton>() {
                    button.release();
                }
            };
            click.connect_released(move |gesture, _, _, _| release(gesture));
            click.connect_cancel(move |gesture, _| release(gesture));
            obj.add_controller(click);

            // By key: from the middle.
            obj.connect_activate(|button| {
                button.press(button.width() as f64 / 2.0, button.height() as f64 / 2.0);
                button.release();
            });
        }
    }

    impl WidgetImpl for RippleButton {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(ripple) = self.ripple.get() {
                let obj = self.obj();
                let (w, h) = (obj.width() as f32, obj.height() as f32);
                let (x, y) = ripple.origin;
                let now = self.now.get();
                // As far as the farthest corner, and its soft edge past it.
                let reach = x.max(w - x).hypot(y.max(h - y)) / 0.7;
                let radius = (reach * ripple.spread(now)).max(1.0);
                let mut color = obj.color();
                color.set_alpha(STRENGTH * ripple.fade(now));
                let mut clear = color;
                clear.set_alpha(0.0);
                snapshot.append_radial_gradient(
                    &graphene::Rect::new(0.0, 0.0, w, h),
                    &graphene::Point::new(x, y),
                    radius,
                    radius,
                    0.0,
                    1.0,
                    &[
                        gsk::ColorStop::new(0.0, color),
                        gsk::ColorStop::new(0.7, color),
                        gsk::ColorStop::new(1.0, clear),
                    ],
                );
            }
            self.parent_snapshot(snapshot);
        }
    }

    impl ButtonImpl for RippleButton {}
}
