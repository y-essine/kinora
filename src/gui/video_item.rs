use gtk::glib::Object;

gtk::glib::wrapper! {
    pub struct VideoItem(ObjectSubclass<imp::VideoItem>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

impl Default for VideoItem {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoItem {
    pub fn new() -> Self {
        let s: Self = Object::builder::<Self>().build();
        s
    }

    /// Kinora: the picture, for the player to grow out of (see `player_sheet.rs`).
    pub fn thumbnail_widget(&self) -> gtk::Picture {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        self.imp().thumbnail.get()
    }
}

pub mod imp {
    use std::cell::Cell;
    use std::cell::RefCell;

    use adw::MultiLayoutView;
    use adw::prelude::AdwDialogExt;
    use chrono::Utc;
    use gdk::Rectangle;
    use gdk::gio;
    use gdk::gio::Settings;
    use gdk::gio::SimpleAction;
    use gdk::gio::SimpleActionGroup;
    use glib::Properties;
    use glib::clone;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::Button;
    use gtk::CompositeTemplate;
    use gtk::Label;
    use gtk::PopoverMenu;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use std::sync::LazyLock;

    use crate::backend::Client;
    use crate::backend::PlayType;
    use crate::backend::Video;
    use crate::config::BASE_ID;
    use crate::gui::filter_similar_dialog::FilterSimilarDialog;
    use crate::gui::utility::Utility;

    #[derive(CompositeTemplate, Properties)]
    #[template(resource = "/ui/video_item.ui")]
    #[properties(wrapper_type = super::VideoItem)]
    pub struct VideoItem {
        #[template_child]
        popover_menu: TemplateChild<PopoverMenu>,
        #[template_child]
        btn_more_actions: TemplateChild<Button>,
        #[template_child]
        multi_layout_view: TemplateChild<MultiLayoutView>,
        #[template_child]
        label_title: TemplateChild<Label>,
        #[template_child]
        avatar: TemplateChild<adw::Avatar>,
        #[template_child]
        clamp_wide: TemplateChild<adw::Clamp>,
        #[template_child]
        slot_image_wide: TemplateChild<adw::LayoutSlot>,
        #[template_child]
        pub(super) thumbnail: TemplateChild<gtk::Picture>,

        /// Kinora: width of the poster card (the "wide" layout); 16:9 follows.
        #[property(get, set = Self::set_card_width, default = 288)]
        card_width: Cell<i32>,

        #[property(get, set)]
        video: RefCell<Option<Video>>,
        #[property(get, set)]
        filter_out_similar_enabled: Cell<bool>,
        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,
        #[property(get, set)]
        hovering: Cell<bool>,

        #[property(get, set)]
        client: RefCell<Client>,

        settings: Settings,
    }

    impl Default for VideoItem {
        fn default() -> Self {
            Self {
                popover_menu: Default::default(),
                btn_more_actions: Default::default(),
                multi_layout_view: Default::default(),
                label_title: Default::default(),
                avatar: Default::default(),
                clamp_wide: Default::default(),
                slot_image_wide: Default::default(),
                thumbnail: Default::default(),
                card_width: Cell::new(288),
                filter_out_similar_enabled: Default::default(),
                compact: Default::default(),
                hovering: Default::default(),
                video: Default::default(),
                client: Default::default(),
                settings: gtk::gio::Settings::new(BASE_ID),
            }
        }
    }

    #[gtk::template_callbacks]
    impl VideoItem {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);

            if compact {
                self.multi_layout_view.set_layout_name("compact");
                self.obj().add_css_class("compact");
                self.avatar.set_size(20);
            } else {
                self.multi_layout_view.set_layout_name("wide");
                self.obj().remove_css_class("compact");
                self.avatar.set_size(32);
            }
        }

        fn set_card_width(&self, width: i32) {
            let width = width.max(120);
            self.card_width.set(width);
            self.clamp_wide.set_maximum_size(width);
            self.slot_image_wide
                .set_size_request(width, (width as f64 * 9.0 / 16.0).round() as i32);
        }

        /// The date after the channel name ("· 3 weeks ago"); on its own line
        /// in the compact layout.
        #[template_callback(function)]
        fn meta_date(date: &str, compact: bool) -> String {
            if date.is_empty() || compact {
                date.to_owned()
            } else {
                format!("· {date}")
            }
        }

        /// Fade a thumbnail in when it arrives (the CSS class carries the
        /// transition); a card that already has one shows it at once.
        fn setup_thumbnail_fade(&self) {
            self.thumbnail.connect_paintable_notify(|thumbnail| {
                if thumbnail.paintable().is_some() {
                    thumbnail.add_css_class("loaded");
                } else {
                    thumbnail.remove_css_class("loaded");
                }
            });
        }

        #[template_callback]
        pub fn handle_hover_enter(&self) {
            self.obj().set_hovering(true);
            self.obj().add_css_class("hovering");
            // Lite mode: nothing is fetched before a click.
            if self.settings.boolean("reduce-effects") {
                return;
            }
            // Kinora: a pointer resting on a video for a moment usually means a
            // click is coming; prepare it so it starts in a second or two.
            let item = glib::prelude::ObjectExt::downgrade(&*self.obj());
            glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                let Some(item) = item.upgrade() else { return };
                if item.hovering() {
                    if let Some(video) = item.video() {
                        item.client().prepare_playback(&video);
                    }
                }
            });
        }

        #[template_callback]
        pub fn handle_hover_leave(&self) {
            let popover_visible = self.popover_menu.is_visible();
            if !popover_visible {
                self.obj().set_hovering(false);
                self.obj().remove_css_class("hovering");
            }
        }

        #[template_callback]
        pub fn handle_right_clicked(&self, _n_presses: i32, x: f64, y: f64) {
            self.open_menu_at(
                &self.create_menu(),
                &Rectangle::new(x as i32, y as i32, 1, 1),
            );
        }

        #[template_callback]
        pub fn handle_long_pressed(&self, x: f64, y: f64) {
            self.open_menu_at(
                &self.create_menu(),
                &Rectangle::new(x as i32, y as i32, 1, 1),
            );
        }

        #[template_callback]
        pub fn handle_more_actions_clicked(&self) {
            let rect = self
                .btn_more_actions
                .compute_bounds(self.obj().as_ref())
                .expect("More actions button to be able to compute bounds");
            self.open_menu_at(
                &self.create_more_actions_menu(),
                &Rectangle::new(
                    rect.x() as i32,
                    rect.y() as i32,
                    rect.width() as i32,
                    rect.height() as i32,
                ),
            );
        }

        fn open_menu_at(&self, menu: &gio::Menu, area: &Rectangle) {
            self.popover_menu.set_menu_model(Some(menu));
            self.popover_menu.set_pointing_to(Some(area));
            self.popover_menu.popup();
        }

        fn setup_actions(&self) {
            let obj = self.obj();

            let action_toggle_watched = SimpleAction::new("toggle-watched", None);
            action_toggle_watched.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    if let Some(video) = obj.video() {
                        obj.client().toggle_watched(video, Utc::now())
                    }
                }
            ));

            let action_watch_later = SimpleAction::new("watch-later", None);
            action_watch_later.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    if let Some(video) = obj.video() {
                        obj.client().toggle_watch_later(video);
                    }
                }
            ));

            let action_play_external = SimpleAction::new("play-inverted", None);
            action_play_external.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.emit_by_name::<()>("play-video", &[&obj.video(), &PlayType::Inverted]);
                }
            ));

            let action_copy_video_url = SimpleAction::new("copy-video-url", None);
            action_copy_video_url.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.emit_by_name::<()>("copy-video-url", &[&obj.video()]);
                }
            ));
            let action_download = SimpleAction::new("download", None);
            action_download.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.emit_by_name::<()>("download", &[&obj.video()]);
                }
            ));
            let action_remove_downloaded = SimpleAction::new("remove-downloaded", None);
            action_remove_downloaded.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.emit_by_name::<()>("remove-downloaded", &[&obj.video()]);
                }
            ));

            let action_filter_out_similar = SimpleAction::new("filter-out-similar", None);
            action_filter_out_similar.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    if let Some(video) = obj.video() {
                        let dialog = FilterSimilarDialog::with_populated_fields_from_video(
                            obj.client(),
                            video,
                        );
                        dialog.present(Some(&obj));
                    }
                }
            ));

            let actions = SimpleActionGroup::new();
            obj.insert_action_group("video-item", Some(&actions));
            actions.add_action(&action_toggle_watched);
            actions.add_action(&action_watch_later);
            actions.add_action(&action_play_external);
            actions.add_action(&action_copy_video_url);
            actions.add_action(&action_download);
            actions.add_action(&action_remove_downloaded);
            actions.add_action(&action_filter_out_similar);
        }

        fn setup_leave_on_popover_close(&self) {
            let obj = self.obj();
            self.popover_menu.connect_closed(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.imp().handle_hover_leave();
                }
            ));
        }

        fn create_menu(&self) -> gio::Menu {
            let menu = gio::Menu::new();
            if self.obj().video().is_some_and(|v| v.watch_later()) {
                menu.append(
                    Some(&gettextrs::gettext("Remove from Watch Later")),
                    Some("video-item.watch-later"),
                );
            } else {
                menu.append(
                    Some(&gettextrs::gettext("Watch Later")),
                    Some("video-item.watch-later"),
                );
            }

            if self.settings.boolean("play-external-default") {
                menu.append(
                    Some(&gettextrs::gettext("Play with Internal Player")),
                    Some("video-item.play-inverted"),
                );
            } else {
                menu.append(
                    Some(&gettextrs::gettext("Play with External Player")),
                    Some("video-item.play-inverted"),
                );
            }

            menu.append(
                Some(&gettextrs::gettext("Copy Video URL")),
                Some("video-item.copy-video-url"),
            );

            if self.obj().video().is_some_and(|d| d.downloaded()) {
                menu.append(
                    Some(&gettextrs::gettext("Remove Local Copy")),
                    Some("video-item.remove-downloaded"),
                );
            } else {
                menu.append(
                    Some(&gettextrs::gettext("Download")),
                    Some("video-item.download"),
                );
            }

            if self.obj().filter_out_similar_enabled() {
                menu.append(
                    Some(&gettextrs::gettext("Filter out Similar Videos")),
                    Some("video-item.filter-out-similar"),
                );
            }

            if self.settings.boolean("watch-history-enabled") {
                if self.obj().video().is_some_and(|v| v.watched_at().is_none()) {
                    menu.append(
                        Some(&gettextrs::gettext("Mark as watched")),
                        Some("video-item.toggle-watched"),
                    );
                } else {
                    menu.append(
                        Some(&gettextrs::gettext("Remove from history")),
                        Some("video-item.toggle-watched"),
                    );
                }
            }

            menu
        }

        fn create_more_actions_menu(&self) -> gio::Menu {
            let menu = gio::Menu::new();
            if self.settings.boolean("play-external-default") {
                menu.append(
                    Some(&gettextrs::gettext("Play with Internal Player")),
                    Some("video-item.play-inverted"),
                );
            } else {
                menu.append(
                    Some(&gettextrs::gettext("Play with External Player")),
                    Some("video-item.play-inverted"),
                );
            }

            menu.append(
                Some(&gettextrs::gettext("Copy Video URL")),
                Some("video-item.copy-video-url"),
            );

            if self.obj().video().is_some_and(|d| d.downloaded()) {
                menu.append(
                    Some(&gettextrs::gettext("Remove Local Copy")),
                    Some("video-item.remove-downloaded"),
                );
            } else {
                menu.append(
                    Some(&gettextrs::gettext("Download")),
                    Some("video-item.download"),
                );
            }

            if self.obj().filter_out_similar_enabled() {
                menu.append(
                    Some(&gettextrs::gettext("Filter out Similar Videos")),
                    Some("video-item.filter-out-similar"),
                );
            }

            if self.settings.boolean("watch-history-enabled") {
                if self.obj().video().is_some_and(|v| v.watched_at().is_none()) {
                    menu.append(
                        Some(&gettextrs::gettext("Mark as watched")),
                        Some("video-item.toggle-watched"),
                    );
                } else {
                    menu.append(
                        Some(&gettextrs::gettext("Remove from history")),
                        Some("video-item.toggle-watched"),
                    );
                }
            }

            menu
        }

        async fn on_long_mapped(&self) {
            let obj = self.obj();
            if let Some(video) = obj.video()
                && video.thumbnail().is_none()
            {
                obj.client().populate_thumbnail(&video).await;
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VideoItem {
        const NAME: &'static str = "TFVideoItem";
        type Type = super::VideoItem;
        type ParentType = gtk::Box;

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
    impl ObjectImpl for VideoItem {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup_actions();
            self.setup_leave_on_popover_close();
            self.setup_thumbnail_fade();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("play-video")
                        .param_types([Video::static_type(), PlayType::static_type()])
                        .build(),
                    Signal::builder("copy-video-url")
                        .param_types([Video::static_type()])
                        .build(),
                    Signal::builder("download")
                        .param_types([Video::static_type()])
                        .build(),
                    Signal::builder("remove-downloaded")
                        .param_types([Video::static_type()])
                        .build(),
                ]
            });
            SIGNALS.as_ref()
        }
    }

    impl WidgetImpl for VideoItem {
        fn map(&self) {
            self.parent_map();
            let obj = self.obj();
            // Load thumbnail of video if mapped for a bit (grid views mapps everything for a very short time initially).
            crate::gspawn!(
                clone!(
                    #[weak]
                    obj,
                    async move {
                        glib::timeout_future(std::time::Duration::from_millis(50)).await;
                        if obj.is_mapped() {
                            obj.imp().on_long_mapped().await;
                        }
                    },
                ),
                glib::Priority::LOW
            );
        }
    }
    impl BoxImpl for VideoItem {}
}
