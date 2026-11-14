gtk::glib::wrapper! {
    pub struct BottomBar(ObjectSubclass<imp::BottomBar>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

pub mod imp {
    use std::cell::Cell;
    use std::cell::RefCell;

    use adw::subclass::bin::BinImpl;
    use chrono::TimeDelta;
    use gdk::gio::SimpleAction;
    use gdk::gio::SimpleActionGroup;
    use glib::Properties;
    use glib::clone;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::CompositeTemplate;
    use gtk::Label;
    use gtk::Picture;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use std::sync::LazyLock;

    use crate::gui::utility::Utility;
    use crate::gui::video_page::VideoPage;

    #[derive(CompositeTemplate, Properties, Default)]
    #[template(resource = "/ui/bottom_bar.ui")]
    #[properties(wrapper_type = super::BottomBar)]
    pub struct BottomBar {
        #[template_child]
        wrapping_box: TemplateChild<gtk::Box>,
        #[template_child]
        thumbnail: TemplateChild<Picture>,
        #[template_child]
        label_uploader: TemplateChild<Label>,
        #[template_child]
        label_duration: TemplateChild<Label>,

        #[property(get, set, nullable)]
        video_page: RefCell<Option<VideoPage>>,
        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,
    }

    #[gtk::template_callbacks]
    impl BottomBar {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);

            if compact {
                self.wrapping_box.set_margin_top(0);
                self.wrapping_box.set_margin_bottom(0);
                self.wrapping_box.set_margin_end(6);
                self.wrapping_box.set_margin_start(0);
                self.thumbnail.remove_css_class("card");
                self.label_uploader.set_visible(false);
                self.label_duration.set_visible(false);
            } else {
                self.wrapping_box.set_margin_top(6);
                self.wrapping_box.set_margin_bottom(6);
                self.wrapping_box.set_margin_end(18);
                self.wrapping_box.set_margin_start(18);
                self.thumbnail.add_css_class("card");
                self.label_uploader.set_visible(true);
                self.label_duration.set_visible(true);
            }
        }

        #[template_callback(function)]
        fn play_pause_icon(state: clapper::PlayerState) -> &'static str {
            match state {
                clapper::PlayerState::Playing => "media-playback-pause-symbolic",
                clapper::PlayerState::Paused => "media-playback-start-symbolic",
                clapper::PlayerState::Buffering => "view-more-horizontal-symbolic",
                // Should never happen
                clapper::PlayerState::Stopped => "media-playback-stop-symbolic",
                _ => "media-playback-stop-symbolic",
            }
        }

        #[template_callback(function)]
        fn play_pause_sensitive(state: clapper::PlayerState) -> bool {
            state == clapper::PlayerState::Playing || state == clapper::PlayerState::Paused
        }

        #[template_callback(function)]
        fn play_pause_tooltip(state: clapper::PlayerState) -> String {
            match state {
                clapper::PlayerState::Playing => gettextrs::gettext("Pause Video"),
                clapper::PlayerState::Paused => gettextrs::gettext("Resume Video"),
                clapper::PlayerState::Buffering => gettextrs::gettext("Video is Buffering"),
                // Should never happen
                clapper::PlayerState::Stopped => gettextrs::gettext("Video is Stopped"),
                _ => gettextrs::gettext("Video is Stopped"),
            }
        }

        #[template_callback(function)]
        fn format_duration(position: f64, duration: f64) -> String {
            format!(
                "{} / {}",
                Utility::format_duration_human(TimeDelta::seconds(position as i64)),
                Utility::format_duration_human(TimeDelta::seconds(duration as i64))
            )
        }

        #[template_callback(function)]
        fn tooltip(title: String) -> String {
            gettextrs::gettext("Expand Video \"{title}\"").replace("{title}", &title)
        }

        #[template_callback]
        fn clicked(&self) {
            self.obj().emit_by_name::<()>("activated", &[]);
        }

        #[template_callback]
        fn dismissed(&self) {
            self.obj().emit_by_name::<()>("dismissed", &[]);
        }

        #[template_callback]
        fn play_pause(&self) {
            self.obj().emit_by_name::<()>("play-pause", &[]);
        }

        fn setup_actions(&self) {
            let obj = self.obj();

            let action_clicked = SimpleAction::new("clicked", None);
            action_clicked.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.imp().clicked();
                }
            ));

            let actions = SimpleActionGroup::new();
            obj.insert_action_group("bottom-bar", Some(&actions));
            actions.add_action(&action_clicked);
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BottomBar {
        const NAME: &'static str = "TFBottomBar";
        type Type = super::BottomBar;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
            Utility::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for BottomBar {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup_actions();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("activated").build(),
                    Signal::builder("dismissed").build(),
                    Signal::builder("play-pause").build(),
                ]
            });
            SIGNALS.as_ref()
        }
    }

    impl WidgetImpl for BottomBar {}
    impl BinImpl for BottomBar {}
}
