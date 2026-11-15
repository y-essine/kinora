//! Kinora: the player sheet (see `docs/design.md`, "Player").
//!
//! One video page lives in this sheet, over the rest of the window. A single
//! number, `progress`, says where it is: 1 is the full page, 0 is the
//! mini-player docked at the bottom. In between, the page is drawn scaled so
//! that its player lands exactly on the mini-player's video slot (or, while
//! opening, on the card that was clicked); nothing is laid out again, so the
//! transition costs one transform per frame. Drags, touchpad swipes and the
//! spring animation all only move `progress`.

use glib::subclass::types::ObjectSubclassIsExt;
use gtk::glib;
use gtk::graphene;
use gtk::prelude::*;

use crate::gui::video_page::VideoPage;

glib::wrapper! {
    pub struct PlayerSheet(ObjectSubclass<imp::PlayerSheet>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PlayerSheet {
    /// Show `page` in the sheet, replacing the page it had (which is paused).
    ///
    /// With `expand`, the page opens in full: out of `origin` (the clicked
    /// card's picture, in the sheet's coordinates) if given, else rising from
    /// the bottom. Without it, a minimized sheet stays minimized.
    pub fn open(&self, page: &VideoPage, origin: Option<graphene::Rect>, expand: bool) {
        self.open_with(page, origin, expand, false);
    }

    /// Like [`Self::open`], but the page that is replaced keeps playing and is
    /// handed back: the DJ fades it out while the new one fades in (see
    /// `gui/dj.rs`), then pauses and drops it.
    pub fn open_over(&self, page: &VideoPage, expand: bool) -> Option<VideoPage> {
        self.open_with(page, None, expand, true)
    }

    fn open_with(
        &self,
        page: &VideoPage,
        origin: Option<graphene::Rect>,
        expand: bool,
        keep_playing: bool,
    ) -> Option<VideoPage> {
        let imp = self.imp();
        imp.stop_animations();
        imp.closing.set(false);
        imp.offset_x.set(0.0);

        let old = imp.video_page.take();
        let was_expanded = old.is_some() && imp.progress.get() > 0.99;
        let was_minimized = old.is_some() && !was_expanded;
        if let Some(old) = &old {
            imp.release_page(old, !keep_playing);
        }

        page.set_parent(self);
        let handlers = vec![
            page.connect_position_notify(glib::clone!(
                #[weak(rename_to = sheet)]
                self,
                move |page| sheet
                    .imp()
                    .mini_progress
                    .set_fraction(page.progress_fraction())
            )),
            page.connect_fullscreened_notify(glib::clone!(
                #[weak(rename_to = sheet)]
                self,
                move |_| sheet.queue_allocate()
            )),
        ];
        imp.page_handlers.replace(handlers);
        imp.mini_progress.set_fraction(0.0);
        imp.video_page.replace(Some(page.clone()));
        self.notify_video_page();
        self.set_visible(true);

        if expand && was_expanded {
            imp.progress.set(1.0);
            imp.origin.set(imp::Origin::Mini);
            imp.set_expanded_state(true);
            imp.apply();
        } else if expand {
            imp.origin.set(match origin {
                Some(r) => imp::Origin::Rect([r.x(), r.y(), r.width(), r.height()]),
                None if was_minimized => imp::Origin::Mini,
                None => imp::Origin::Below,
            });
            imp.progress.set(0.0);
            imp.apply();
            imp.animate_to(1.0, 0.0);
        } else {
            imp.progress.set(0.0);
            imp.origin.set(imp::Origin::Mini);
            imp.set_expanded_state(false);
            imp.apply();
        }
        old.filter(|_| keep_playing)
    }

    /// Shrink the page into the mini-player; the video keeps playing.
    pub fn minimize(&self) {
        let imp = self.imp();
        let Some(page) = self.video_page() else {
            return;
        };
        if !self.expanded() || imp.closing.get() {
            return;
        }
        if page.fullscreened() {
            page.set_fullscreened(false);
        }
        page.scroll_top_now();
        imp.origin.set(imp::Origin::Mini);
        imp.animate_to(0.0, 0.0);
    }

    /// Bring the mini-player back to the full page.
    pub fn expand(&self) {
        let imp = self.imp();
        if self.video_page().is_none() || self.expanded() || imp.closing.get() {
            return;
        }
        imp.origin.set(imp::Origin::Mini);
        imp.animate_offset(0.0, false);
        imp.animate_to(1.0, 0.0);
    }

    pub fn toggle(&self) {
        if self.expanded() {
            self.minimize();
        } else {
            self.expand();
        }
    }

    /// Close the player: the video is paused and the sheet goes away.
    pub fn close(&self) {
        let imp = self.imp();
        if self.video_page().is_none() || imp.closing.get() {
            return;
        }
        if self.expanded() {
            imp.finish_close();
        } else {
            imp.slide_out(if imp.compact.get() || imp.dock_right.get() {
                1.0
            } else {
                -1.0
            });
        }
    }

    /// Test hook: hold the sheet at a point of its transition.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_set_progress(&self, progress: f64) {
        let imp = self.imp();
        imp.stop_animations();
        imp.origin.set(imp::Origin::Mini);
        imp.progress.set(progress);
        imp.apply();
    }

    /// Test hook: dock the mini-player on the left or the right.
    #[cfg(feature = "dev-hooks")]
    pub fn dev_set_dock_right(&self, right: bool) {
        self.imp().dock_right.set(right);
        self.imp().apply();
    }
}

pub mod imp {
    use std::cell::{Cell, RefCell};
    use std::sync::LazyLock;

    use adw::prelude::*;
    use glib::subclass::{InitializingObject, Signal};
    use glib::{Properties, clone};
    use gtk::subclass::prelude::*;
    use gtk::{CompositeTemplate, glib, graphene, gsk};

    use crate::config::BASE_ID;
    use crate::gui::utility::Utility;
    use crate::gui::video_page::VideoPage;

    /// Mini-player as a card (wide windows): its width, the height of the row
    /// under the video, and the gap to the window's edges.
    const CARD_WIDTH: f64 = 336.0;
    const CARD_INFO_HEIGHT: f64 = 60.0;
    const CARD_MARGIN: f64 = 20.0;
    /// Mini-player as a bar (narrow windows).
    const BAR_HEIGHT: f64 = 64.0;
    const BAR_MARGIN: f64 = 8.0;
    const BAR_SLOT_WIDTH: f64 = (BAR_HEIGHT * 16.0 / 9.0) as i32 as f64;
    /// Corner radius of the mini-player (`--k-radius-m`).
    const MINI_RADIUS: f32 = 12.0;
    /// A touchpad swipe moves the sheet a little further than the fingers.
    const SWIPE_GAIN: f64 = 1.6;
    /// Sideways swipe on the player, in pixels, that skips to the next video.
    const SKIP_DISTANCE: f64 = 140.0;

    /// Where the video comes from, or goes to, at `progress` 0.
    #[derive(Clone, Copy, PartialEq, Debug, Default)]
    pub enum Origin {
        /// The mini-player's video slot.
        #[default]
        Mini,
        /// A picture elsewhere in the window (the card that was clicked).
        Rect([f32; 4]),
        /// Below the window: the page rises from the bottom.
        Below,
    }

    #[derive(Clone, Copy, PartialEq, Debug, Default)]
    enum Lock {
        /// No gesture.
        #[default]
        None,
        /// Began; its direction isn't known yet.
        Pending,
        /// Moves `progress`.
        Vertical,
        /// Moves the mini-player sideways.
        Horizontal,
        /// Sideways over the player: next or previous video.
        Skip,
        /// Not for the sheet.
        Pass,
    }

    /// A pointer drag or a touchpad swipe in progress.
    #[derive(Default)]
    struct Gesture {
        lock: Lock,
        from_expanded: bool,
        /// The page was scrolled to its top when the gesture began.
        at_top: bool,
        moved: bool,
        /// Pixels between the expanded and the minimized video.
        travel: f64,
        start_offset: f64,
        /// Where the direction was decided (drag), or the distance so far (swipe).
        x: f64,
        y: f64,
        /// (time in µs, value) of the last moments, for the release velocity.
        samples: Vec<(i64, f64)>,
    }

    impl Gesture {
        fn sample(&mut self, value: f64) {
            let now = glib::monotonic_time();
            self.samples.push((now, value));
            self.samples.retain(|(t, _)| now - t <= 120_000);
        }

        /// Units per second over the last moments.
        fn velocity(&self) -> f64 {
            let now = glib::monotonic_time();
            let recent: Vec<_> = self
                .samples
                .iter()
                .filter(|(t, _)| now - t <= 150_000)
                .collect();
            match (recent.first(), recent.last()) {
                (Some((t0, v0)), Some((t1, v1))) if t1 - t0 >= 8_000 => {
                    (v1 - v0) / ((t1 - t0) as f64 / 1_000_000.0)
                }
                _ => 0.0,
            }
        }
    }

    pub(super) fn smoothstep(from: f64, to: f64, x: f64) -> f64 {
        let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Past its ends, `progress` follows the hand with growing resistance.
    pub(super) fn rubber_band(progress: f64) -> f64 {
        const LIMIT: f64 = 0.05;
        let resist = |over: f64| LIMIT * (1.0 - 1.0 / (over / LIMIT * 0.55 + 1.0));
        if progress > 1.0 {
            1.0 + resist(progress - 1.0)
        } else if progress < 0.0 {
            -resist(-progress)
        } else {
            progress
        }
    }

    #[derive(CompositeTemplate, Properties)]
    #[template(resource = "/ui/player_sheet.ui")]
    #[properties(wrapper_type = super::PlayerSheet)]
    pub struct PlayerSheet {
        #[template_child]
        backdrop: TemplateChild<gtk::Box>,
        #[template_child]
        mini: TemplateChild<gtk::Overlay>,
        #[template_child]
        mini_layout: TemplateChild<gtk::Box>,
        #[template_child]
        mini_slot: TemplateChild<gtk::Box>,
        #[template_child]
        mini_info: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) mini_progress: TemplateChild<gtk::ProgressBar>,

        /// The page in the sheet, if any.
        #[property(get, nullable)]
        pub(super) video_page: RefCell<Option<VideoPage>>,
        /// The page is (or is on its way to being) shown in full.
        #[property(get)]
        expanded: Cell<bool>,
        /// Narrow window: the mini-player is a bar across the bottom.
        #[property(get, set = Self::set_compact)]
        pub(super) compact: Cell<bool>,
        /// Height taken at the bottom by the window's own bar.
        #[property(get, set = Self::set_bottom_inset)]
        bottom_inset: Cell<i32>,

        /// 0: minimized. 1: expanded.
        pub(super) progress: Cell<f64>,
        pub(super) origin: Cell<Origin>,
        pub(super) dock_right: Cell<bool>,
        /// The mini-player's sideways displacement while it is swiped.
        pub(super) offset_x: Cell<f64>,
        pub(super) closing: Cell<bool>,

        animation: RefCell<Option<adw::Animation>>,
        offset_animation: RefCell<Option<adw::Animation>>,
        pub(super) page_handlers: RefCell<Vec<glib::SignalHandlerId>>,

        drag: RefCell<Gesture>,
        swipe: RefCell<Gesture>,
        swipe_timeout: RefCell<Option<glib::SourceId>>,
        pointer: Cell<(f64, f64)>,
        /// Where the page's player was laid out last.
        player_rect: Cell<[f64; 4]>,
        clip: RefCell<Option<gsk::RoundedRect>>,

        settings: gtk::gio::Settings,
    }

    impl Default for PlayerSheet {
        fn default() -> Self {
            Self {
                backdrop: Default::default(),
                mini: Default::default(),
                mini_layout: Default::default(),
                mini_slot: Default::default(),
                mini_info: Default::default(),
                mini_progress: Default::default(),
                video_page: Default::default(),
                expanded: Default::default(),
                compact: Default::default(),
                bottom_inset: Default::default(),
                progress: Default::default(),
                origin: Default::default(),
                dock_right: Cell::new(true),
                offset_x: Default::default(),
                closing: Default::default(),
                animation: Default::default(),
                offset_animation: Default::default(),
                page_handlers: Default::default(),
                drag: Default::default(),
                swipe: Default::default(),
                swipe_timeout: Default::default(),
                pointer: Default::default(),
                player_rect: Default::default(),
                clip: Default::default(),
                settings: gtk::gio::Settings::new(BASE_ID),
            }
        }
    }

    #[gtk::template_callbacks]
    impl PlayerSheet {
        #[template_callback(function)]
        fn play_pause_icon(state: clapper::PlayerState) -> &'static str {
            match state {
                clapper::PlayerState::Playing => "media-playback-pause-symbolic",
                _ => "media-playback-start-symbolic",
            }
        }

        #[template_callback(function)]
        fn play_pause_tooltip(state: clapper::PlayerState) -> String {
            match state {
                clapper::PlayerState::Playing => gettextrs::gettext("Pause"),
                clapper::PlayerState::Buffering => gettextrs::gettext("Video is Buffering"),
                _ => gettextrs::gettext("Play"),
            }
        }

        #[template_callback(function)]
        fn expand_tooltip(title: Option<String>) -> String {
            gettextrs::gettext("Expand “{title}”").replace("{title}", &title.unwrap_or_default())
        }

        #[template_callback]
        fn play_pause(&self) {
            if let Some(page) = self.obj().video_page() {
                page.play_pause();
            }
        }

        #[template_callback]
        fn close_clicked(&self) {
            self.obj().close();
        }

        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);
            if compact {
                self.mini.add_css_class("bar");
                self.mini_layout
                    .set_orientation(gtk::Orientation::Horizontal);
                self.mini_slot.set_size_request(BAR_SLOT_WIDTH as i32, -1);
                self.mini_slot.set_vexpand(false);
                self.mini_info.set_size_request(-1, -1);
            } else {
                self.mini.remove_css_class("bar");
                self.mini_layout.set_orientation(gtk::Orientation::Vertical);
                self.mini_slot.set_size_request(-1, -1);
                self.mini_slot.set_vexpand(true);
                self.mini_info.set_size_request(-1, CARD_INFO_HEIGHT as i32);
            }
            self.offset_x.set(0.0);
            self.apply();
        }

        fn set_bottom_inset(&self, inset: i32) {
            if self.bottom_inset.replace(inset) != inset {
                self.obj().queue_allocate();
            }
        }

        fn reduce_effects(&self) -> bool {
            self.settings.boolean("reduce-effects")
        }

        // --- Geometry -------------------------------------------------------

        /// The mini-player's place without its sideways displacement:
        /// x, y, width, height.
        fn mini_base(&self, width: f64, height: f64) -> [f64; 4] {
            let inset = self.bottom_inset.get() as f64;
            if self.compact.get() {
                let w = (width - 2.0 * BAR_MARGIN).max(1.0);
                [
                    BAR_MARGIN,
                    height - inset - BAR_MARGIN - BAR_HEIGHT,
                    w,
                    BAR_HEIGHT,
                ]
            } else {
                let w = CARD_WIDTH.min(width - 2.0 * CARD_MARGIN).max(1.0);
                let h = (w * 9.0 / 16.0).round() + CARD_INFO_HEIGHT;
                let x = if self.dock_right.get() {
                    width - CARD_MARGIN - w
                } else {
                    CARD_MARGIN
                };
                [x, height - inset - CARD_MARGIN - h, w, h]
            }
        }

        fn mini_rect(&self, width: f64, height: f64) -> [f64; 4] {
            let [x, y, w, h] = self.mini_base(width, height);
            [(x + self.offset_x.get()).round(), y.round(), w, h]
        }

        /// The video's place inside the mini-player.
        fn slot_rect(&self, mini: [f64; 4]) -> [f64; 4] {
            let [x, y, w, h] = mini;
            if self.compact.get() {
                [x, y, BAR_SLOT_WIDTH, h]
            } else {
                [x, y, w, h - CARD_INFO_HEIGHT]
            }
        }

        fn in_mini(&self, x: f64, y: f64) -> bool {
            let obj = self.obj();
            let [mx, my, mw, mh] = self.mini_rect(obj.width() as f64, obj.height() as f64);
            x >= mx && x <= mx + mw && y >= my && y <= my + mh
        }

        /// How far the video's centre travels between expanded and minimized.
        fn travel(&self) -> f64 {
            let obj = self.obj();
            let [_, py, _, ph] = self.player_rect.get();
            let mini = self.mini_rect(obj.width() as f64, obj.height() as f64);
            let [_, sy, _, sh] = self.slot_rect(mini);
            ((sy + sh / 2.0) - (py + ph / 2.0)).abs().max(160.0)
        }

        /// 1 while the mini-player is in place, less as it is swiped away.
        fn presence(&self) -> f64 {
            let offset = self.offset_x.get();
            let width = self.obj().width().max(1) as f64;
            let gone = if self.compact.get() {
                offset.abs() / (width * 0.6)
            } else {
                let outward = if self.dock_right.get() {
                    offset
                } else {
                    -offset
                };
                outward / (CARD_WIDTH * 0.9)
            };
            1.0 - gone.clamp(0.0, 1.0)
        }

        /// Bring every part in line with `progress` and `offset_x`.
        pub(super) fn apply(&self) {
            let obj = self.obj();
            let p = self.progress.get().clamp(0.0, 1.0);
            let from_mini = self.origin.get() == Origin::Mini;
            let presence = self.presence();
            self.backdrop.set_opacity(smoothstep(0.1, 0.6, p));
            self.mini.set_opacity(if from_mini {
                (1.0 - smoothstep(0.0, 0.3, p)) * presence
            } else {
                0.0
            });
            if let Some(page) = self.video_page.borrow().as_ref() {
                page.set_chrome_opacity(smoothstep(0.5, 1.0, p));
                page.set_opacity(if p < 0.01 { presence } else { 1.0 });
            }
            obj.queue_allocate();
            obj.queue_draw();
        }

        // --- State ------------------------------------------------------------

        pub(super) fn set_expanded_state(&self, expanded: bool) {
            let obj = self.obj();
            let Some(page) = obj.video_page() else {
                return;
            };
            page.set_can_target(expanded);
            page.set_minimized(!expanded);
            self.mini.set_can_target(!expanded);
            if expanded {
                page.set_can_focus(true);
                page.grab_focus();
            } else {
                // The keyboard goes back to what is behind the sheet.
                if let Some(root) = obj.root()
                    && root.focus().is_some_and(|f| f.is_ancestor(&page))
                {
                    root.set_focus(None::<&gtk::Widget>);
                }
                page.set_can_focus(false);
            }
            if self.expanded.replace(expanded) != expanded {
                obj.notify_expanded();
            }
        }

        pub(super) fn stop_animations(&self) {
            if let Some(animation) = self.animation.take() {
                animation.pause();
            }
            if let Some(animation) = self.offset_animation.take() {
                animation.pause();
            }
        }

        /// A new gesture takes over from whatever was moving.
        fn begin_interaction(&self) {
            self.stop_animations();
            if self.origin.replace(Origin::Mini) != Origin::Mini {
                self.apply();
            }
        }

        fn animation(
            &self,
            from: f64,
            to: f64,
            velocity: f64,
            target: adw::CallbackAnimationTarget,
        ) -> adw::Animation {
            let obj = self.obj();
            if self.reduce_effects() {
                let animation = adw::TimedAnimation::new(&*obj, from, to, 140, target);
                animation.set_easing(adw::Easing::EaseOutCubic);
                animation.upcast()
            } else {
                // Critically damped: it arrives and stops, without a bounce.
                let animation = adw::SpringAnimation::new(
                    &*obj,
                    from,
                    to,
                    adw::SpringParams::new(1.0, 1.0, 320.0),
                    target,
                );
                animation.set_initial_velocity(velocity);
                animation.set_epsilon(0.0005);
                animation.upcast()
            }
        }

        /// Move `progress` to 0 or 1, starting at the hand's `velocity`.
        pub(super) fn animate_to(&self, to: f64, velocity: f64) {
            let obj = self.obj();
            if let Some(animation) = self.animation.take() {
                animation.pause();
            }
            self.set_expanded_state(to > 0.5);
            let from = self.progress.get();
            if (from - to).abs() < 0.0005 {
                self.settled(to);
                return;
            }
            let target = adw::CallbackAnimationTarget::new(clone!(
                #[weak]
                obj,
                move |value| {
                    obj.imp().progress.set(value);
                    obj.imp().apply();
                }
            ));
            let animation = self.animation(from, to, velocity, target);
            animation.connect_done(clone!(
                #[weak]
                obj,
                move |_| obj.imp().settled(to)
            ));
            self.animation.replace(Some(animation.clone()));
            animation.play();
        }

        fn settled(&self, at: f64) {
            self.animation.take();
            self.progress.set(at);
            self.origin.set(Origin::Mini);
            self.apply();
        }

        /// Move the mini-player's displacement to `to`, then close if asked.
        pub(super) fn animate_offset(&self, to: f64, then_close: bool) {
            let obj = self.obj();
            if let Some(animation) = self.offset_animation.take() {
                animation.pause();
            }
            let from = self.offset_x.get();
            if (from - to).abs() < 0.5 && !then_close {
                self.offset_x.set(to);
                self.apply();
                return;
            }
            let target = adw::CallbackAnimationTarget::new(clone!(
                #[weak]
                obj,
                move |value| {
                    obj.imp().offset_x.set(value);
                    obj.imp().apply();
                }
            ));
            let animation = self.animation(from, to, 0.0, target);
            animation.connect_done(clone!(
                #[weak]
                obj,
                move |_| {
                    let imp = obj.imp();
                    imp.offset_animation.take();
                    if then_close {
                        imp.finish_close();
                    }
                }
            ));
            self.offset_animation.replace(Some(animation.clone()));
            animation.play();
        }

        /// The mini-player leaves over the window's edge (`direction` -1 or 1).
        pub(super) fn slide_out(&self, direction: f64) {
            self.closing.set(true);
            let distance = if self.compact.get() {
                self.obj().width() as f64 * 0.6
            } else {
                CARD_WIDTH
            };
            self.animate_offset(direction * distance, true);
        }

        pub(super) fn release_page(&self, page: &VideoPage, pause: bool) {
            for handler in self.page_handlers.take() {
                page.disconnect(handler);
            }
            // Only pause: stopping Clapper here has crashed (see VideoPage::stop).
            if pause {
                page.stop();
            }
            page.unparent();
        }

        pub(super) fn finish_close(&self) {
            let obj = self.obj();
            self.stop_animations();
            let Some(page) = self.video_page.take() else {
                return;
            };
            if page.fullscreened() {
                page.set_fullscreened(false);
            }
            self.release_page(&page, true);
            self.closing.set(false);
            self.progress.set(0.0);
            self.offset_x.set(0.0);
            self.origin.set(Origin::Mini);
            self.clip.replace(None);
            if self.expanded.replace(false) {
                obj.notify_expanded();
            }
            obj.set_visible(false);
            obj.notify_video_page();
            obj.emit_by_name::<()>("closed", &[&page]);
        }

        // --- Gestures -----------------------------------------------------------

        /// The pointer is on something with its own dragging or clicking.
        fn on_control(&self, x: f64, y: f64) -> bool {
            let obj = self.obj();
            let mut widget = obj.pick(x, y, gtk::PickFlags::DEFAULT);
            while let Some(w) = widget {
                if w == *obj.upcast_ref::<gtk::Widget>() {
                    break;
                }
                let name = w.type_().name();
                if w.is::<gtk::Button>()
                    || w.is::<gtk::MenuButton>()
                    || w.is::<gtk::Range>()
                    || w.is::<gtk::Switch>()
                    || w.is::<gtk::Popover>()
                    || name == "ClapperGtkSimpleControls"
                    || name == "ClapperGtkSeekBar"
                {
                    return true;
                }
                widget = w.parent();
            }
            false
        }

        /// Release: go to the nearer end, or where the hand was heading.
        fn settle(&self, from_expanded: bool, velocity: f64) {
            let p = self.progress.get();
            let expand = if velocity.abs() > 1.0 {
                velocity > 0.0
            } else if from_expanded {
                p > 0.72
            } else {
                p > 0.28
            };
            self.animate_to(if expand { 1.0 } else { 0.0 }, velocity);
        }

        /// The mini-player was let go after a sideways move at `velocity` px/s:
        /// over the near edge it closes, towards the middle it changes sides.
        fn release_sideways(&self, velocity: f64) {
            let obj = self.obj();
            let offset = self.offset_x.get();
            if self.compact.get() {
                if offset.abs() > obj.width() as f64 * 0.3 || velocity.abs() > 800.0 {
                    let direction = if velocity.abs() > 800.0 {
                        velocity
                    } else {
                        offset
                    };
                    self.slide_out(direction.signum());
                } else {
                    self.animate_offset(0.0, false);
                }
                return;
            }
            let edge = if self.dock_right.get() { 1.0 } else { -1.0 };
            let outward = offset * edge;
            let outward_velocity = velocity * edge;
            if outward > 0.0 && (outward > CARD_WIDTH * 0.4 || outward_velocity > 800.0) {
                self.slide_out(edge);
            } else if outward < 0.0 && (-outward > 110.0 || -outward_velocity > 800.0) {
                // To the other bottom corner.
                let (width, height) = (obj.width() as f64, obj.height() as f64);
                let x = self.mini_base(width, height)[0] + offset;
                self.dock_right.set(!self.dock_right.get());
                self.offset_x.set(x - self.mini_base(width, height)[0]);
                self.animate_offset(0.0, false);
            } else {
                self.animate_offset(0.0, false);
            }
        }

        fn drag_begin(&self, gesture: &gtk::GestureDrag, x: f64, y: f64) {
            let obj = self.obj();
            let page = obj.video_page();
            let eligible = match page {
                Some(_) if self.closing.get() => false,
                Some(page) if self.expanded.get() => {
                    self.progress.get() > 0.99
                        && !page.fullscreened()
                        && page.stage_contains(x, y)
                        && !self.on_control(x, y)
                }
                Some(_) => self.in_mini(x, y) && !self.on_control(x, y),
                None => false,
            };
            if !eligible {
                self.drag.replace(Gesture::default());
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            self.drag.replace(Gesture {
                lock: Lock::Pending,
                from_expanded: self.expanded.get(),
                travel: self.travel(),
                ..Default::default()
            });
        }

        fn drag_update(&self, gesture: &gtk::GestureDrag, ox: f64, oy: f64) {
            let mut drag = self.drag.borrow_mut();
            if drag.lock == Lock::Pending {
                if drag.from_expanded {
                    // Only a clear pull down takes the video along.
                    if oy > 10.0 && oy > ox.abs() * 1.2 {
                        drag.lock = Lock::Vertical;
                    } else if ox.abs() > 16.0 || oy < -12.0 {
                        drag.lock = Lock::None;
                        drop(drag);
                        gesture.set_state(gtk::EventSequenceState::Denied);
                        return;
                    }
                } else if ox.abs() > 8.0 || oy.abs() > 8.0 {
                    drag.moved = true;
                    drag.lock = if ox.abs() > oy.abs() {
                        Lock::Horizontal
                    } else if oy < 0.0 {
                        Lock::Vertical
                    } else {
                        Lock::Pass
                    };
                }
                if matches!(drag.lock, Lock::Vertical | Lock::Horizontal) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    drag.x = ox;
                    drag.y = oy;
                    drag.start_offset = self.offset_x.get();
                    drop(drag);
                    self.begin_interaction();
                    drag = self.drag.borrow_mut();
                }
            }
            match drag.lock {
                Lock::Vertical => {
                    let moved = (oy - drag.y) / drag.travel;
                    let p = rubber_band(if drag.from_expanded {
                        1.0 - moved
                    } else {
                        -moved
                    });
                    drag.sample(p);
                    drop(drag);
                    self.progress.set(p);
                    self.apply();
                }
                Lock::Horizontal => {
                    let offset = drag.start_offset + ox - drag.x;
                    drag.sample(offset);
                    drop(drag);
                    self.offset_x.set(offset);
                    self.apply();
                }
                _ => {}
            }
        }

        fn drag_end(&self, cancelled: bool) {
            let drag = self.drag.take();
            match drag.lock {
                // A click on the mini-player.
                Lock::Pending if !drag.from_expanded && !drag.moved && !cancelled => {
                    self.obj().expand();
                }
                Lock::Vertical => self.settle(drag.from_expanded, drag.velocity()),
                Lock::Horizontal => self.release_sideways(drag.velocity()),
                _ => {}
            }
        }

        fn swipe_begin(&self) {
            let obj = self.obj();
            let Some(page) = obj.video_page() else {
                return;
            };
            if self.closing.get() {
                return;
            }
            self.swipe.replace(Gesture {
                lock: Lock::Pending,
                from_expanded: self.expanded.get(),
                at_top: page.scroll_position() < 1.0,
                ..Default::default()
            });
        }

        fn swipe_scroll(
            &self,
            controller: &gtk::EventControllerScroll,
            dx: f64,
            dy: f64,
        ) -> glib::Propagation {
            let obj = self.obj();
            // Only touchpads: a mouse wheel has no begin and no end.
            if controller.unit() != gtk::gdk::ScrollUnit::Surface {
                return glib::Propagation::Proceed;
            }
            let Some(page) = obj.video_page() else {
                return glib::Propagation::Proceed;
            };
            let mut swipe = self.swipe.borrow_mut();
            if swipe.lock == Lock::Pending {
                let (x, y) = self.pointer.get();
                swipe.lock = if dx == 0.0 && dy == 0.0 {
                    Lock::Pending
                } else if swipe.from_expanded {
                    if page.fullscreened() || self.progress.get() < 0.99 {
                        Lock::Pass
                    } else if swipe.at_top && dy < 0.0 && dy.abs() > dx.abs() {
                        // Pulling down at the top of the page.
                        Lock::Vertical
                    } else if dx.abs() > dy.abs() * 2.0
                        && page.stage_contains(x, y)
                        && !self.on_control(x, y)
                    {
                        Lock::Skip
                    } else {
                        Lock::Pass
                    }
                } else if !self.in_mini(x, y) {
                    Lock::Pass
                } else if dx.abs() > dy.abs() {
                    Lock::Horizontal
                } else if dy > 0.0 {
                    Lock::Vertical
                } else {
                    Lock::Pass
                };
                if matches!(swipe.lock, Lock::Vertical | Lock::Horizontal) {
                    swipe.start_offset = self.offset_x.get();
                    drop(swipe);
                    self.begin_interaction();
                    let travel = self.travel();
                    swipe = self.swipe.borrow_mut();
                    swipe.travel = travel;
                }
            }
            let handled = match swipe.lock {
                Lock::Vertical => {
                    swipe.y += dy * SWIPE_GAIN;
                    let moved = swipe.y / swipe.travel;
                    let p = rubber_band(if swipe.from_expanded {
                        1.0 + moved
                    } else {
                        moved
                    });
                    swipe.sample(p);
                    drop(swipe);
                    self.progress.set(p);
                    self.apply();
                    true
                }
                Lock::Horizontal => {
                    swipe.x += dx * SWIPE_GAIN;
                    let offset = swipe.start_offset - swipe.x;
                    swipe.sample(offset);
                    drop(swipe);
                    self.offset_x.set(offset);
                    self.apply();
                    true
                }
                Lock::Skip => {
                    swipe.x += dx;
                    true
                }
                _ => false,
            };
            if !handled {
                return glib::Propagation::Proceed;
            }
            // Should the end of the swipe never arrive, don't stay in between.
            if let Some(source) = self.swipe_timeout.take() {
                source.remove();
            }
            let source = glib::timeout_add_local_once(
                std::time::Duration::from_millis(1500),
                clone!(
                    #[weak]
                    obj,
                    move || {
                        obj.imp().swipe_timeout.take();
                        obj.imp().swipe_end();
                    }
                ),
            );
            self.swipe_timeout.replace(Some(source));
            glib::Propagation::Stop
        }

        fn swipe_end(&self) {
            if let Some(source) = self.swipe_timeout.take() {
                source.remove();
            }
            let swipe = self.swipe.take();
            match swipe.lock {
                Lock::Vertical => self.settle(swipe.from_expanded, swipe.velocity()),
                Lock::Horizontal => self.release_sideways(swipe.velocity()),
                Lock::Skip if swipe.x.abs() > SKIP_DISTANCE => {
                    // Fingers to the left: the next video comes in from the right.
                    self.obj().emit_by_name::<()>("skip", &[&(swipe.x > 0.0)]);
                }
                _ => {}
            }
        }

        fn setup_gestures(&self) {
            let obj = self.obj();

            // In the capture phase and on the sheet itself, which stays put
            // while the page and the mini-player move under the pointer.
            let drag = gtk::GestureDrag::new();
            drag.set_propagation_phase(gtk::PropagationPhase::Capture);
            drag.connect_drag_begin(clone!(
                #[weak]
                obj,
                move |gesture, x, y| obj.imp().drag_begin(gesture, x, y)
            ));
            drag.connect_drag_update(clone!(
                #[weak]
                obj,
                move |gesture, x, y| obj.imp().drag_update(gesture, x, y)
            ));
            drag.connect_drag_end(clone!(
                #[weak]
                obj,
                move |_, _, _| obj.imp().drag_end(false)
            ));
            drag.connect_cancel(clone!(
                #[weak]
                obj,
                move |_, _| obj.imp().drag_end(true)
            ));
            obj.add_controller(drag);

            let scroll =
                gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
            scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
            scroll.connect_scroll_begin(clone!(
                #[weak]
                obj,
                move |_| obj.imp().swipe_begin()
            ));
            scroll.connect_scroll(clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |controller, dx, dy| obj.imp().swipe_scroll(controller, dx, dy)
            ));
            scroll.connect_scroll_end(clone!(
                #[weak]
                obj,
                move |_| obj.imp().swipe_end()
            ));
            obj.add_controller(scroll);

            let motion = gtk::EventControllerMotion::new();
            motion.set_propagation_phase(gtk::PropagationPhase::Capture);
            motion.connect_motion(clone!(
                #[weak]
                obj,
                move |_, x, y| obj.imp().pointer.set((x, y))
            ));
            obj.add_controller(motion);
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlayerSheet {
        const NAME: &'static str = "TFPlayerSheet";
        type Type = super::PlayerSheet;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
            Utility::bind_template_callbacks(klass);
            klass.set_css_name("playersheet");
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for PlayerSheet {
        fn constructed(&self) {
            self.parent_constructed();
            self.set_compact(false);
            self.setup_gestures();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    // The player was closed; the page is no longer in the sheet.
                    Signal::builder("closed")
                        .param_types([VideoPage::static_type()])
                        .build(),
                    // Next (true) or previous (false) video requested.
                    Signal::builder("skip")
                        .param_types([bool::static_type()])
                        .build(),
                ]
            });
            SIGNALS.as_ref()
        }

        fn dispose(&self) {
            self.stop_animations();
            if let Some(source) = self.swipe_timeout.take() {
                source.remove();
            }
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for PlayerSheet {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            // As large as it is given; the children are still asked, as GTK
            // wants every widget measured before it is given a size.
            let mut child = self.obj().first_child();
            while let Some(c) = child {
                let _ = c.measure(orientation, for_size);
                child = c.next_sibling();
            }
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let (w, h) = (width as f64, height as f64);
            self.backdrop.allocate(width, height, -1, None);

            let mini = self.mini_rect(w, h);
            let _ = self.mini.measure(gtk::Orientation::Horizontal, -1);
            self.mini.allocate(
                mini[2] as i32,
                mini[3] as i32,
                -1,
                Some(
                    gsk::Transform::new()
                        .translate(&graphene::Point::new(mini[0] as f32, mini[1] as f32)),
                ),
            );

            let Some(page) = self.video_page.borrow().clone() else {
                return;
            };
            let _ = page.measure(gtk::Orientation::Horizontal, -1);
            // Laid out in full, always; the transform alone moves it.
            page.allocate(width, height, -1, None);
            let p = self.progress.get();
            let origin = self.origin.get();
            let player = page.player_rect().unwrap_or([0.0, 0.0, w, h]);
            self.player_rect.set(player);
            if (p - 1.0).abs() < 0.0001 || page.fullscreened() || player[2] < 1.0 {
                self.clip.replace(None);
                return;
            }
            let collapsed = match origin {
                Origin::Mini => self.slot_rect(mini),
                Origin::Rect(r) => [r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64],
                Origin::Below => [player[0], player[1] + h, player[2], player[3]],
            };
            let lerp = |a: f64, b: f64| a + (b - a) * p;
            let x = lerp(collapsed[0], player[0]);
            let y = lerp(collapsed[1], player[1]);
            let scale = lerp(collapsed[2], player[2]) / player[2];
            // The player isn't always exactly as wide for its height as where
            // it is going: what is too much is cut off evenly.
            let shown_height = lerp(collapsed[3], player[3]).min(player[3] * scale);
            let y = y - (player[3] * scale - shown_height) / 2.0;
            page.allocate(
                width,
                height,
                -1,
                Some(
                    gsk::Transform::new()
                        .translate(&graphene::Point::new(
                            (x - player[0] * scale) as f32,
                            (y - player[1] * scale) as f32,
                        ))
                        .scale(scale as f32, scale as f32),
                ),
            );

            // Once the page around it has faded, the video alone is left: it
            // takes the corners of where it is going.
            let clip = (p < 0.5 && origin != Origin::Below).then(|| {
                let bounds = graphene::Rect::new(
                    x as f32,
                    lerp(collapsed[1], player[1]) as f32,
                    (player[2] * scale) as f32,
                    shown_height as f32,
                );
                let round = graphene::Size::new(MINI_RADIUS, MINI_RADIUS);
                let square = graphene::Size::zero();
                match origin {
                    Origin::Mini if self.compact.get() => {
                        gsk::RoundedRect::new(bounds, round, square, square, round)
                    }
                    Origin::Mini => gsk::RoundedRect::new(bounds, round, round, square, square),
                    _ => gsk::RoundedRect::new(bounds, round, round, round, round),
                }
            });
            self.clip.replace(clip);
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            obj.snapshot_child(&*self.backdrop, snapshot);
            obj.snapshot_child(&*self.mini, snapshot);
            if let Some(page) = self.video_page.borrow().as_ref() {
                let clip = self.clip.borrow();
                if let Some(clip) = clip.as_ref() {
                    snapshot.push_rounded_clip(clip);
                    obj.snapshot_child(page, snapshot);
                    snapshot.pop();
                } else {
                    obj.snapshot_child(page, snapshot);
                }
            }
        }

        /// Only the page and the mini-player take the pointer; everywhere
        /// else it goes to what is behind the sheet.
        fn contains(&self, _x: f64, _y: f64) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::imp::{rubber_band, smoothstep};

    #[test]
    fn rubber_band_leaves_the_range_alone() {
        for p in [0.0, 0.25, 0.5, 1.0] {
            assert_eq!(rubber_band(p), p);
        }
    }

    #[test]
    fn rubber_band_gives_a_little_past_the_ends() {
        // It follows, less and less, and never further than 5 %.
        assert!(rubber_band(1.02) > 1.0 && rubber_band(1.02) < 1.02);
        assert!(rubber_band(1.2) > rubber_band(1.1));
        assert!(rubber_band(50.0) < 1.05);
        assert!(rubber_band(-0.02) < 0.0 && rubber_band(-0.02) > -0.02);
        assert!(rubber_band(-50.0) > -0.05);
    }

    #[test]
    fn smoothstep_is_clamped_and_eased() {
        assert_eq!(smoothstep(0.5, 1.0, 0.2), 0.0);
        assert_eq!(smoothstep(0.5, 1.0, 1.3), 1.0);
        assert_eq!(smoothstep(0.0, 1.0, 0.5), 0.5);
        assert!(smoothstep(0.0, 1.0, 0.25) < 0.25);
    }
}
