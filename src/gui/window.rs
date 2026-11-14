use adw::Toast;
use adw::prelude::GtkApplicationExt;
use gdk::gio::prelude::SettingsExtManual;
use gdk::subclass::prelude::ObjectSubclassIsExt;
use gtk::glib::Object;
use gtk::prelude::GtkWindowExt;
use gtk::prelude::SettingsExt;

gtk::glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, adw::Window, gtk::Window, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Window {
    pub fn new(app: &gtk::Application) -> Self {
        app.set_accels_for_action("win.settings", &["<Control>comma"]);
        app.set_accels_for_action("win.about", &["F1"]);
        app.set_accels_for_action("win.show-help-overlay", &["<Control>question"]);
        app.set_accels_for_action("win.search", &["<Ctrl>f", "F3", "<Alt>2"]);
        app.set_accels_for_action("win.subscribe-or-watch-later", &["<Ctrl>d"]);
        app.set_accels_for_action("window.close", &["<Control>q", "<Control>w"]);
        app.set_accels_for_action("win.reload", &["<Control>r", "F5"]);
        app.set_accels_for_action("win.toggle-sidebar", &["F9"]);
        // Kinora: minimize or bring back the player.
        app.set_accels_for_action("win.toggle-player", &["<Control>i"]);

        app.set_accels_for_action("win.tab-feed", &["<Alt>1"]);
        app.set_accels_for_action("win.tab-watch-later", &["<Alt>3"]);
        app.set_accels_for_action("win.tab-subscriptions", &["<Alt>4"]);

        Object::builder::<Self>()
            .property("application", app)
            .build()
    }

    fn save_window_size(&self) -> Result<(), gtk::glib::BoolError> {
        let imp = self.imp();

        let (width, height) = self.default_size();

        imp.settings.set_int("window-width", width)?;
        imp.settings.set_int("window-height", height)?;

        imp.settings
            .set_boolean("is-maximized", self.is_maximized())?;

        Ok(())
    }

    fn load_window_size(&self) {
        let imp = self.imp();

        let width = imp.settings.int("window-width");
        let height = imp.settings.int("window-height");
        let is_maximized = imp.settings.boolean("is-maximized");

        self.set_default_size(width, height);

        if is_maximized {
            self.maximize();
        }
    }

    fn debug_info(&self) -> String {
        let settings = &self.imp().settings;
        let mut lines = vec![];
        lines.push("- Versions:".to_string());
        lines.push(format!(
            "    - Version: {} ({}; {})",
            env!("CARGO_PKG_VERSION"),
            if crate::config::FLATPAK {
                "Flatpak"
            } else {
                "Not Flatpak"
            },
            if crate::config::PROFILE == "Devel" {
                "Development Version"
            } else {
                "Release Version"
            }
        ));
        lines.push(format!(
            "    - GTK version: {}.{}.{}",
            gtk::major_version(),
            gtk::minor_version(),
            gtk::micro_version()
        ));
        lines.push(format!(
            "    - Libadwaita version: {}.{}.{}",
            adw::major_version(),
            adw::minor_version(),
            adw::micro_version()
        ));

        lines.push("- Preferences:".to_string());
        lines.push(format!(
            "    - YouTube Enabled: {}",
            settings.boolean("enable-source-youtube")
        ));
        lines.push(format!(
            "    - PeerTube Enabled: {}",
            settings.boolean("enable-source-peertube")
        ));

        let youtube_mode = settings.string("youtube-mode");
        lines.push(format!("    - YouTube Method: {}", youtube_mode));
        if youtube_mode.contains("piped") {
            lines.push(format!(
                "    - Piped instances managed automatically: {}",
                settings.boolean("piped-automatically-manage-instances")
            ));
            lines.push("    - Piped instances:".to_string());
            for instance in settings.get::<Vec<String>>("piped-api-urls") {
                lines.push(format!("        - {instance}"));
            }
        }
        lines.push(format!(
            "    - YouTube Cookies Set: {}",
            clapper::global_enhancer_proxies()
                .and_then(|p| p.proxy_by_module("clapper_yt_dlp"))
                .and_then(|p| p.settings())
                .map(|s| s.string("cookies-file"))
                .map(|c| if c.is_empty() { "None" } else { "Set" })
                .unwrap_or("Cannot access Clapper settings")
        ));
        lines.push(format!(
            "    - Peertube search API: {}",
            settings.string("peertube-search-api")
        ));
        lines.push(format!(
            "    - External player by default: {}",
            settings.boolean("play-external-default")
        ));
        lines.push(format!(
            "    - External player: {}",
            settings.string("player")
        ));
        lines.push(format!(
            "    - Cairo rendering: {}",
            settings.boolean("cairo-software-renderer")
        ));

        lines.join("\n")
    }

    pub fn display_error(&self, text: String, error: Box<dyn std::error::Error>) {
        // Ignore an error which happens whenever a YouTube video is opened and which therefore is very annoying.
        // See the upstream error: https://github.com/TeamPiped/Piped/issues/3658.
        if format!("{error:?}").contains("Sign in to confirm that you're not a bot") {
            return;
        }

        // Ignore Piped internal server errors; nothing we can do about those.
        if format!("{error:?}").contains("status code 500 Internal Server Error") {
            return;
        }

        // Ignore errors where the provider was not found.
        // This most likely means the user disabled a provider but still interacted with a video or channel of such a provider.
        // TODO: Maybe consider making it impossible in the first place to interact with such videos or channels, but that is likely pretty hard to do.
        if format!("{error:?}").starts_with("ProviderSpecific(ProviderNotFound") {
            return;
        }

        let imp = self.imp();
        let mut errors = imp.errors.borrow_mut();
        errors.push((text.clone(), error));

        let num_errors = errors.len();

        let toast_title = if num_errors == 1 {
            text
        } else {
            gettextrs::ngettext("One Error", "{} Errors", num_errors as u32)
                .replace("{}", &num_errors.to_string())
        };

        imp.toast_error.set_title(&toast_title);

        self.show_toast(imp.toast_error.get());
    }

    pub fn show_toast(&self, toast: Toast) {
        self.imp().toast_overlay.add_toast(toast);
    }
}

pub mod imp {
    use std::cell::{Cell, RefCell};
    use std::path::{Path, PathBuf};

    use crate::backend::{
        Channel, Client, PlayType, SearchStream, SubscriptionsStore, Video, VideoFetcherStream,
    };
    use crate::config::{self, BASE_ID, PROFILE};
    use crate::gui::channel_item::ChannelItem;
    use crate::gui::channel_page::ChannelPage;
    use crate::gui::error_dialog::ErrorDialog;
    use crate::gui::export_dialog::ExportDialog;
    use crate::gui::filter_dialog::FilterDialog;
    use crate::gui::finite_video_list::FiniteVideoList;
    use crate::gui::import_dialog::ImportDialog;
    use crate::gui::player_sheet::PlayerSheet;
    use crate::gui::preferences_dialog::PreferencesDialog;
    use crate::gui::privacy_notice::PrivacyNotice;
    use crate::gui::search_list::SearchList;
    use crate::gui::subscriptions_list::SubscriptionsList;
    use crate::gui::utility::Utility;
    use crate::gui::video_item::VideoItem;
    use crate::gui::video_list::VideoList;
    use crate::gui::video_page::VideoPage;
    use crate::{gspawn, tspawn};

    use adw::prelude::{AdwApplicationWindowExt, AdwDialogExt, NavigationPageExt};
    use adw::{
        NavigationPage, NavigationView, OverlaySplitView, Toast, ToastOverlay, ViewStack,
        ViewStackPage, ViewSwitcherBar, WindowTitle,
    };
    use chrono::Utc;
    use futures::StreamExt;
    use gdk::gio::{MenuModel, SettingsBindFlags, SimpleAction, SimpleActionGroup};
    use glib::subclass::InitializingObject;
    use glib::{Properties, Variant, clone};
    use gtk::accessible::Relation;
    use gtk::glib::Propagation;
    use gtk::subclass::prelude::*;
    use gtk::{
        AccessibleAnnouncementPriority, Builder, Button, Label, ListBox, ListBoxRow, Overlay,
        ProgressBar, SearchEntry, glib,
    };
    use gtk::{Image, Orientation, prelude::*};

    use adw::ShortcutsDialog;
    use adw::subclass::prelude::AdwApplicationWindowImpl;
    use adw::subclass::prelude::AdwWindowImpl;
    use gtk::CompositeTemplate;

    const MOBILE_BOTTOM_BAR_ITEMS: u32 = 3;

    #[derive(CompositeTemplate, Properties)]
    #[properties(wrapper_type = super::Window)]
    #[template(resource = "/ui/window.ui")]
    pub struct Window {
        #[template_child]
        main_stack: TemplateChild<ViewStack>,
        #[template_child]
        sidebar_list: TemplateChild<ListBox>,

        #[template_child]
        main_title: TemplateChild<WindowTitle>,
        #[template_child]
        main_navigation: TemplateChild<NavigationView>,
        #[template_child]
        main_page: TemplateChild<NavigationPage>,
        #[template_child]
        view_switcher: TemplateChild<ViewSwitcherBar>,

        #[template_child]
        split_view: TemplateChild<OverlaySplitView>,
        #[template_child]
        video_list: TemplateChild<VideoList>,

        #[template_child]
        search_entry: TemplateChild<SearchEntry>,
        // Kinora: source filter of the Search page.
        #[template_child]
        search_source: TemplateChild<adw::ToggleGroup>,
        #[template_child]
        search_list: TemplateChild<SearchList>,
        #[template_child]
        search_page: TemplateChild<ViewStackPage>,

        #[template_child]
        btn_filter_menu: TemplateChild<Button>,

        #[template_child]
        label_splash: TemplateChild<Label>,
        #[template_child]
        overlay_videos: TemplateChild<Overlay>,
        // Kinora: the player sheet and what it lies over (see docs/design.md).
        #[template_child]
        player_sheet: TemplateChild<PlayerSheet>,
        #[template_child]
        player_overlay: TemplateChild<Overlay>,
        /// Videos played before the current one in this sitting ("previous").
        player_history: RefCell<Vec<Video>>,

        #[template_child]
        menubar: TemplateChild<MenuModel>,

        #[template_child]
        pub(super) toast_overlay: TemplateChild<ToastOverlay>,
        #[template_child]
        pub(super) toast_error: TemplateChild<Toast>,

        #[template_child]
        progress_feed: TemplateChild<ProgressBar>,

        // Kinora: Home (banner and rows).
        #[template_child]
        home_view: TemplateChild<crate::gui::home_view::HomeView>,

        pub(super) errors: RefCell<Vec<(String, Box<dyn std::error::Error>)>>,

        #[property(get)]
        client: RefCell<Client>,

        #[property(get, set)]
        split_view_collapsed_before_fullscreen: Cell<bool>,
        #[property(get, set = Self::set_breakpoint_collapsed)]
        breakpoint_collapsed: Cell<bool>,
        #[property(get, set, default = true)]
        sidebar_pinned: Cell<bool>,

        #[property(get, set)]
        main_show_sidebar_visible: Cell<bool>,

        #[property(get, set)]
        watch_history_enabled: Cell<bool>,

        // Kinora: the DJ (see `gui/dj.rs`).
        dj: std::cell::OnceCell<crate::gui::dj::Dj>,
        /// A DJ session is running.
        #[property(get, set)]
        dj_on: Cell<bool>,
        /// The DJ extension is switched on.
        #[property(get, set)]
        dj_available: Cell<bool>,

        pub settings: gtk::gio::Settings,
    }

    impl Default for Window {
        fn default() -> Self {
            Self {
                main_stack: Default::default(),
                sidebar_list: Default::default(),
                main_title: Default::default(),
                main_navigation: Default::default(),
                main_page: Default::default(),
                split_view: Default::default(),
                view_switcher: Default::default(),
                video_list: Default::default(),
                search_page: Default::default(),
                search_entry: Default::default(),
                search_source: Default::default(),
                search_list: Default::default(),
                btn_filter_menu: Default::default(),
                label_splash: Default::default(),
                overlay_videos: Default::default(),
                toast_overlay: Default::default(),
                toast_error: Default::default(),
                player_sheet: Default::default(),
                player_overlay: Default::default(),
                player_history: Default::default(),
                menubar: Default::default(),
                progress_feed: Default::default(),
                home_view: Default::default(),

                errors: Default::default(),

                settings: gtk::gio::Settings::new(BASE_ID),

                split_view_collapsed_before_fullscreen: Default::default(),
                breakpoint_collapsed: Default::default(),
                sidebar_pinned: Default::default(),
                main_show_sidebar_visible: Default::default(),
                watch_history_enabled: Default::default(),
                dj: Default::default(),
                dj_on: Default::default(),
                dj_available: Default::default(),

                client: Default::default(),
            }
        }
    }

    #[gtk::template_callbacks]
    impl Window {
        #[template_callback(function)]
        fn feed_visible(page: Option<&str>) -> bool {
            page == Some("feed")
        }

        #[template_callback(function)]
        fn home_visible(page: Option<&str>) -> bool {
            page == Some("home")
        }

        #[template_callback(function)]
        fn reload_visible(page: Option<&str>) -> bool {
            page == Some("feed") || page == Some("home")
        }

        #[template_callback(function)]
        fn is_video_visible(page: Option<&str>) -> bool {
            page == Some("video-page")
        }

        #[template_callback(function)]
        fn is_channel_visible(page: Option<&str>) -> bool {
            page == Some("channel-page")
        }

        #[template_callback(function)]
        fn refresh_button_visible_child_name(loading: bool) -> &'static str {
            if loading { "spinner" } else { "button" }
        }

        #[template_callback(function)]
        fn pin_label(pinned: bool) -> String {
            if pinned {
                gettextrs::gettext("Unpin Sidebar")
            } else {
                gettextrs::gettext("Pin Sidebar")
            }
        }

        /// Kinora: the player was closed from its mini-player.
        #[template_callback]
        fn player_closed(&self, page: VideoPage) {
            if let Some(video) = page.video() {
                self.obj().client().watched(video, Utc::now());
            }
            self.player_history.borrow_mut().clear();
            self.dj().leave();
        }

        /// Kinora: the next video (the first of "Up next") or the one before.
        #[template_callback]
        fn player_skip(&self, forward: bool) {
            let Some(page) = self.player_sheet.video_page() else {
                return;
            };
            // With the DJ on, "next" skips the track; there is no way back.
            if self.dj().is_on() {
                if forward {
                    self.dj().skip();
                }
                return;
            }
            if forward {
                if let Some(next) = page.next_video() {
                    self.open_video(next, PlayType::Default, true, true);
                }
            } else {
                let previous = self.player_history.borrow_mut().pop();
                if let Some(previous) = previous {
                    self.open_video(previous, PlayType::Default, true, false);
                }
            }
        }

        /// Kinora: where a video's picture is shown behind the sheet (its card
        /// or the banner), in the sheet's coordinates: the player grows out of it.
        fn card_rect(&self, video: &Video) -> Option<gtk::graphene::Rect> {
            use crate::gui::featured_banner::FeaturedBanner;
            let overlay = self.player_overlay.get();
            let area =
                gtk::graphene::Rect::new(0.0, 0.0, overlay.width() as f32, overlay.height() as f32);
            let mut found = None;
            let mut pending = vec![self.main_navigation.get().upcast::<gtk::Widget>()];
            while let Some(widget) = pending.pop() {
                if !widget.is_mapped() {
                    continue;
                }
                let picture = if let Some(item) = widget.downcast_ref::<VideoItem>() {
                    (item.video().as_ref() == Some(video)).then(|| item.thumbnail_widget())
                } else if let Some(banner) = widget.downcast_ref::<FeaturedBanner>() {
                    (banner.video().as_ref() == Some(video)).then(|| banner.picture_widget())
                } else {
                    let mut child = widget.first_child();
                    while let Some(c) = child {
                        child = c.next_sibling();
                        pending.push(c);
                    }
                    continue;
                };
                let Some(bounds) = picture.and_then(|p| p.compute_bounds(&overlay)) else {
                    continue;
                };
                // On screen for the most part.
                let visible = bounds
                    .intersection(&area)
                    .is_some_and(|i| i.area() > bounds.area() * 0.6);
                if !visible || bounds.width() < 40.0 {
                    continue;
                }
                // The same video can be in several rows: the one under the pointer.
                if widget.state_flags().contains(gtk::StateFlags::PRELIGHT) {
                    return Some(bounds);
                }
                found.get_or_insert(bounds);
            }
            found
        }

        /// Kinora: a page for `video`, wired to the window. `dj`: a DJ track's
        /// page (see `gui/dj.rs`).
        fn new_video_page(&self, video: &Video, dj: bool) -> VideoPage {
            let obj = self.obj();
            let video_page = if dj {
                VideoPage::new_dj(video, &obj.client())
            } else {
                VideoPage::new(video, &obj.client())
            };
            obj.bind_property("dj-available", &video_page, "dj-available")
                .sync_create()
                .build();

            obj.announce(
                &gettextrs::gettext("Switched to video page {}").replace("{}", &video.title()),
                AccessibleAnnouncementPriority::Medium,
            );

            obj.bind_property(
                "main-show-sidebar-visible",
                &video_page,
                "show-sidebar-visible",
            )
            .sync_create()
            .build();

            self.split_view
                .bind_property("show-sidebar", &video_page, "primary-menu-visible")
                .sync_create()
                .invert_boolean()
                .build();

            video_page.connect_local(
                "display-channel",
                false,
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_default]
                    move |args| {
                        let channel: Channel = args[1]
                            .get()
                            .expect("display-channel argument to be of type Channel");
                        obj.imp().display_channel(channel);
                        None
                    }
                ),
            );

            video_page.connect_local(
                "download-video",
                false,
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_default]
                    move |args| {
                        let video: Video = args[1]
                            .get()
                            .expect("download-video argument to be of type Video");
                        obj.imp().download(video);
                        None
                    }
                ),
            );

            video_page.connect_local(
                "remove-downloaded",
                false,
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_default]
                    move |args| {
                        let video: Video = args[1]
                            .get()
                            .expect("remove-downloaded argument to be of type Video");
                        obj.imp().remove_downloaded(video);
                        None
                    }
                ),
            );

            video_page.connect_local(
                "copy-video-url-to-clipboard",
                false,
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_default]
                    move |args| {
                        let video: Video = args[1]
                            .get()
                            .expect("copy-video-url-to-clipboard argument to be of type Video");
                        obj.imp().copy_video_url(video);
                        None
                    }
                ),
            );

            // Kinora: a video picked from "Up next", or autoplay.
            video_page.connect_local(
                "play-video",
                false,
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_default]
                    move |args| {
                        let video: Video = args[1]
                            .get()
                            .expect("play-video argument to be of type Video");
                        let play_type: PlayType = args[2]
                            .get()
                            .expect("play-video argument to be of type PlayType");
                        obj.imp().open_video(video, play_type, true, true);
                        None
                    }
                ),
            );

            video_page.connect_fullscreened_notify(clone!(
                #[weak]
                obj,
                #[upgrade_or_default]
                move |page| {
                    let fullscreen = page.fullscreened();
                    // Kinora: window fullscreen keeps the window as it is.
                    obj.set_fullscreened(fullscreen && !page.window_mode());
                    if fullscreen {
                        obj.set_split_view_collapsed_before_fullscreen(
                            obj.imp().split_view.is_collapsed(),
                        );
                        obj.imp().split_view.set_show_sidebar(false);
                        obj.imp().view_switcher.set_reveal(false);
                    } else if obj.current_breakpoint().is_some() {
                        obj.imp().split_view.set_show_sidebar(false);
                        obj.imp().view_switcher.set_reveal(true);
                    } else {
                        obj.imp()
                            .split_view
                            .set_show_sidebar(obj.split_view_collapsed_before_fullscreen());
                        obj.imp().view_switcher.set_reveal(false);
                    }
                }
            ));

            video_page
        }

        /// Kinora: the DJ's next track takes the player (see `gui/dj.rs`). The
        /// sheet stays as it is, expanded or minimized. With `over`, the page
        /// it replaces keeps playing and is returned, for the DJ to fade out.
        pub(crate) fn open_dj_track(
            &self,
            video: &Video,
            over: bool,
        ) -> (VideoPage, Option<VideoPage>) {
            let obj = self.obj();
            let sheet = self.player_sheet.get();
            let current = sheet.video_page();
            let mut carry_fullscreen = None;
            if let Some(old) = &current {
                if let Some(old_video) = old.video() {
                    obj.client().watched(old_video, Utc::now());
                }
                if old.fullscreened() {
                    carry_fullscreen = Some(old.window_mode());
                }
            }
            let expand = current.is_none() || sheet.expanded();
            let page = self.new_video_page(video, true);
            let outgoing = if over {
                sheet.open_over(&page, expand)
            } else {
                sheet.open(&page, None, expand);
                None
            };
            drop(current);
            if let Some(window_mode) = carry_fullscreen {
                page.set_window_mode(window_mode);
                page.set_fullscreened(true);
            }
            if expand {
                page.grab_focus();
            }
            page.play();
            (page, outgoing)
        }

        pub(crate) fn current_video_page(&self) -> Option<VideoPage> {
            self.player_sheet.video_page()
        }

        pub(crate) fn expand_player(&self) {
            self.player_sheet.expand();
        }

        fn dj(&self) -> &crate::gui::dj::Dj {
            self.dj.get().expect("The DJ to be set up")
        }

        #[template_callback]
        fn play_video(&self, video: Video, play_type: PlayType) {
            self.open_video(video, play_type, false, true);
        }

        /// Kinora: play a video in the player sheet (see docs/design.md, "Player").
        /// There is one player: a new video takes the place of the current one.
        ///
        /// `from_player`: asked for by the player itself (Up next, autoplay,
        /// next or previous), so a minimized player stays minimized.
        /// `remember`: the current video becomes the "previous" one.
        fn open_video(&self, video: Video, play_type: PlayType, from_player: bool, remember: bool) {
            let obj = self.obj();
            let sheet = self.player_sheet.get();

            let play_is_default = play_type == PlayType::Default;
            let play_external_default = self.settings.boolean("play-external-default");
            let play_internal = play_is_default ^ play_external_default;

            let current = sheet.video_page();

            // If the same video is already playing, bring it back instead of starting it again.
            if let Some(page) = &current
                && page.video().as_ref() == Some(&video)
                && play_internal
                && !page.external()
                && !page.finished()
            {
                sheet.expand();
                return;
            }

            // Anything else that is opened ends the DJ: back to ordinary playing.
            self.dj().leave();

            // The old one counts as watched.
            let mut carry_fullscreen = None;
            if let Some(old) = &current {
                if let Some(old_video) = old.video() {
                    obj.client().watched(old_video.clone(), Utc::now());
                    if from_player && remember {
                        self.player_history.borrow_mut().push(old_video);
                    }
                }
                if old.fullscreened() {
                    carry_fullscreen = Some(old.window_mode());
                }
            }
            if !from_player {
                self.player_history.borrow_mut().clear();
            }

            let was_minimized = current.is_some() && !sheet.expanded();
            let expand = !(from_player && was_minimized);
            let origin = if expand && !from_player {
                self.card_rect(&video)
            } else {
                None
            };

            log::debug!(
                "Player: opening {} (expand: {expand}, out of a card: {})",
                video.id().id,
                origin.is_some()
            );

            let video_page = self.new_video_page(&video, false);

            // The old page is paused and let go, and has to be gone before the
            // new player starts: an old player ending while a new one starts
            // has frozen the app (see also VideoPage::stop).
            sheet.open(&video_page, origin, expand);
            drop(current);

            // Autoplay in fullscreen stays in fullscreen.
            if let Some(window_mode) = carry_fullscreen {
                video_page.set_window_mode(window_mode);
                video_page.set_fullscreened(true);
            }

            if expand {
                video_page.grab_focus();
            }

            if play_internal {
                video_page.play();
            } else {
                video_page.play_external();
            }
        }

        #[template_callback]
        fn display_channel(&self, channel: Channel) {
            let obj = self.obj();

            // Kinora: the channel opens behind the player, which steps aside.
            self.player_sheet.minimize();

            let channel_page = ChannelPage::new(&channel, &obj.client());
            channel_page.reload();
            let page = NavigationPage::new(&channel_page, &channel.name());
            self.main_navigation_pre_push();
            self.main_navigation.push(&page);

            obj.announce(
                &gettextrs::gettext("Switched to channel page {}").replace("{}", &channel.name()),
                AccessibleAnnouncementPriority::Medium,
            );

            obj.bind_property(
                "main-show-sidebar-visible",
                &channel_page,
                "show-sidebar-visible",
            )
            .sync_create()
            .build();

            obj.bind_property("breakpoint_collapsed", &channel_page, "compact")
                .sync_create()
                .build();

            self.split_view
                .bind_property("show-sidebar", &channel_page, "primary-menu-visible")
                .sync_create()
                .invert_boolean()
                .build();

            channel_page.connect_local(
                "play-video",
                false,
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_default]
                    move |args| {
                        let video: Video = args[1]
                            .get()
                            .expect("play-video argument to be of type Video");
                        let play_type: PlayType = args[2]
                            .get()
                            .expect("play-video 2nd argument to be of type PlayType");
                        obj.imp().play_video(video, play_type);
                        None
                    }
                ),
            );

            channel_page.connect_local(
                "download",
                false,
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_default]
                    move |args| {
                        let video: Video = args[1]
                            .get()
                            .expect("download-video argument to be of type Video");
                        obj.imp().download(video);
                        None
                    }
                ),
            );

            channel_page.connect_local(
                "remove-downloaded",
                false,
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_default]
                    move |args| {
                        let video: Video = args[1]
                            .get()
                            .expect("remove-downloaded argument to be of type Video");
                        obj.imp().remove_downloaded(video);
                        None
                    }
                ),
            );

            channel_page.connect_local(
                "copy-video-url",
                false,
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_default]
                    move |args| {
                        let video: Video = args[1]
                            .get()
                            .expect("copy-video-url-to-clipboard argument to be of type Video");
                        obj.imp().copy_video_url(video);
                        None
                    }
                ),
            );

            // For some reason, immediately grabbing focus does not work.
            // Waiting a bit before grabbing focus seems to work.
            gspawn!(clone!(
                #[weak]
                channel_page,
                async move {
                    glib::timeout_future(std::time::Duration::from_millis(50)).await;
                    channel_page.grab_focus();
                }
            ));
        }

        #[template_callback]
        fn show_unsubscribe_toast(&self, channel: Channel) {
            let obj = self.obj();

            let toast_title = gettextrs::gettext("Unsubscribed from {channel}")
                .replace("{channel}", &channel.name());

            let toast = Toast::builder()
                .title(toast_title)
                .button_label(gettextrs::gettext("Undo"))
                .build();

            toast.connect_button_clicked(clone!(
                #[weak]
                channel,
                #[weak]
                obj,
                move |_| {
                    obj.client().toggle_subscription(channel);
                }
            ));

            obj.show_toast(toast);
        }

        /// Kinora: test hooks (feature `dev-hooks`, development only): drive the
        /// app with environment variables, since a test session can't click.
        #[cfg(feature = "dev-hooks")]
        fn dev_hooks(&self) {
            let obj = self.obj().clone();
            let obj_open = obj.clone();
            if let Ok(page) = std::env::var("KH_PAGE") {
                self.main_stack.set_visible_child_name(&page);
            }
            if let Ok(q) = std::env::var("KH_SEARCH") {
                self.main_stack.set_visible_child_name("search");
                self.search_entry.set_text(&q);
                self.search();
            }
            // Home may still be loading (it starts on its own): wait for it.
            if let Ok(n) = std::env::var("KH_OPEN") {
                let n: u32 = n.parse().unwrap_or(0);
                let mut tries = 0;
                glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
                    tries += 1;
                    let item = obj_open.client().home_store().item(n);
                    match item.and_downcast::<Video>() {
                        Some(v) => {
                            obj_open.imp().play_video(v, PlayType::Default);
                            glib::ControlFlow::Break
                        }
                        None if tries < 120 => glib::ControlFlow::Continue,
                        None => glib::ControlFlow::Break,
                    }
                });
            }
            // `KH_HOME_SCROLL=pixels@seconds`: scroll Home down.
            if let Ok(spec) = std::env::var("KH_HOME_SCROLL") {
                let (y, secs) = spec.split_once('@').unwrap_or((&spec, "1"));
                let (y, secs): (f64, u64) = (y.parse().unwrap_or(0.0), secs.parse().unwrap_or(1));
                let home = self.home_view.get();
                glib::timeout_add_local_once(std::time::Duration::from_secs(secs), move || {
                    home.scroll_to(y);
                });
            }
            // `KH_SHEET_PROGRESS=0.5@12`: hold the player sheet halfway between
            // minimized (0) and expanded (1). `KH_MINI_DOCK=left`: dock it left.
            if let Ok(spec) = std::env::var("KH_SHEET_PROGRESS") {
                let (p, secs) = spec.split_once('@').unwrap_or((&spec, "1"));
                let (p, secs): (f64, u64) = (p.parse().unwrap_or(0.5), secs.parse().unwrap_or(1));
                let sheet = self.player_sheet.get();
                glib::timeout_add_local_once(std::time::Duration::from_secs(secs), move || {
                    sheet.dev_set_progress(p);
                });
            }
            // `KH_PLAYLIST_DEMO=1`: sample cards in Home's Mixes and playlists
            // shelves, to look at them without the account's own.
            if std::env::var("KH_PLAYLIST_DEMO").is_ok() {
                use crate::gui::playlist_shelf::{PlaylistInfo, PlaylistKind};
                let home = self.obj().client().home_store();
                let sample = |n: u32, kind, title: &str, owner: Option<&str>, count| PlaylistInfo {
                    id: format!("DEMO{n}"),
                    title: title.to_owned(),
                    owner: owner.map(str::to_owned),
                    count,
                    kind,
                    thumbnail: home
                        .item(n)
                        .and_downcast::<Video>()
                        .and_then(|v| v.thumbnail_url()),
                };
                self.home_view.set_playlists(
                    vec![
                        sample(1, PlaylistKind::Mix, "My Mix", Some("YouTube"), None),
                        sample(
                            2,
                            PlaylistKind::Mix,
                            "Mix: The Cranberries, Zombie",
                            None,
                            None,
                        ),
                        sample(
                            3,
                            PlaylistKind::Mix,
                            "Late night drive",
                            Some("YouTube"),
                            None,
                        ),
                    ],
                    vec![
                        sample(
                            4,
                            PlaylistKind::Playlist,
                            "Liked videos",
                            Some("You"),
                            Some(214),
                        ),
                        sample(
                            5,
                            PlaylistKind::Playlist,
                            "Watch later",
                            Some("You"),
                            Some(37),
                        ),
                        sample(
                            6,
                            PlaylistKind::Album,
                            "No Need to Argue",
                            Some("The Cranberries"),
                            Some(13),
                        ),
                    ],
                );
            }
            if let Ok(side) = std::env::var("KH_MINI_DOCK") {
                self.player_sheet.dev_set_dock_right(side != "left");
            }
            if let Ok(list) = std::env::var("KH_ACTION") {
                for item in list.split(',') {
                    let (name, secs) = item.split_once('@').unwrap_or((item, "1"));
                    let secs: u64 = secs.parse().unwrap_or(1);
                    let (o, name) = (obj.clone(), name.to_owned());
                    glib::timeout_add_local_once(std::time::Duration::from_secs(secs), move || {
                        // `video.NAME` goes to the open video page's own actions.
                        let (target, full): (gtk::Widget, String) = match name.strip_prefix("video.") {
                            Some(_) => match super::dev_find_video_page(o.upcast_ref()) {
                                Some(page) => (page, name.clone()),
                                None => return,
                            },
                            None => (o.clone().upcast(), format!("win.{name}")),
                        };
                        log::info!("DEV: action {full}");
                        let _ = gtk::prelude::WidgetExt::activate_action(&target, &full, None);
                    });
                }
            }
        }

        /// Kinora: the video page lives in the player sheet, expanded or not.
        fn visible_video_page(&self) -> Option<VideoPage> {
            self.player_sheet.video_page()
        }

        fn visible_channel_page(&self) -> Option<ChannelPage> {
            let page = self.main_navigation.visible_page();
            let child = page.and_then(|p| p.child());
            child.and_dynamic_cast::<ChannelPage>().ok()
        }

        fn main_navigation_pre_push(&self) {
            // Kinora: whatever opens, opens behind the player.
            self.player_sheet.minimize();
        }

        #[template_callback]
        fn search(&self) {
            gspawn!(clone!(
                #[weak(rename_to = obj)]
                self.obj(),
                async move {
                    let s = obj.imp();
                    let text = s.search_entry.text();
                    let platform = s.search_platform();
                    s.search_list
                        .set_model(obj.client().search(&text, platform).await);
                }
            ));
        }

        /// Kinora: the platform to restrict searches to, if any.
        /// The filter only exists while both sources are enabled; with one source enabled, only that one is searched anyway.
        fn search_platform(&self) -> Option<&'static str> {
            if !(self.settings.boolean("enable-source-youtube")
                && self.settings.boolean("enable-source-peertube"))
            {
                return None;
            }
            match self.settings.string("search-source").as_str() {
                "youtube" => Some("youtube"),
                "peertube" => Some("peertube"),
                _ => None,
            }
        }

        /// Kinora: re-run the current search when the source filter changes.
        #[template_callback]
        fn search_source_changed(&self) {
            if self.search_list.model().is_some() && !self.search_entry.text().is_empty() {
                self.search();
            }
        }

        fn setup_search_source(&self) {
            self.settings
                .bind("search-source", &*self.search_source, "active-name")
                .build();

            let update_visibility = clone!(
                #[weak(rename_to = toggle_group)]
                self.search_source,
                move |settings: &gtk::gio::Settings| {
                    toggle_group.set_visible(
                        settings.boolean("enable-source-youtube")
                            && settings.boolean("enable-source-peertube"),
                    );
                }
            );
            update_visibility(&self.settings);
            for key in ["enable-source-youtube", "enable-source-peertube"] {
                self.settings.connect_changed(
                    Some(key),
                    clone!(
                        #[strong]
                        update_visibility,
                        move |settings, _| update_visibility(settings)
                    ),
                );
            }
        }

        #[template_callback]
        fn sidebar_child_changed(&self) {
            let is_search_page = self.main_stack.visible_child_name() == Some("search".into());
            let is_feed_page = self.main_stack.visible_child_name() == Some("feed".into());

            if is_search_page {
                // Cannot focus search bar right now, as sometimes focus is set to other widgets in the code, e.g. to the widget that was actually clicked in the sidebar on bottom bar.
                // I'm not sure whether we can fix that in some other way, but waiting a bit before moving focus works.
                gspawn!(clone!(
                    #[weak(rename_to=search_entry)]
                    self.search_entry,
                    async move {
                        glib::timeout_future(std::time::Duration::from_millis(50)).await;
                        search_entry.grab_focus();
                        search_entry.select_region(0, -1);
                    }
                ));
            }

            self.main_title.set_title(
                &self
                    .main_stack
                    .page(&self.main_stack.visible_child().unwrap())
                    .title()
                    .unwrap(),
            );

            self.main_navigation.pop_to_tag("root");
            // Kinora: going somewhere else keeps the video, as a mini-player.
            self.player_sheet.minimize();

            let obj = self.obj();
            obj.action_set_enabled("win.filter-enabled", is_feed_page);
            obj.action_set_enabled("win.filter-dialog", is_feed_page);

            if let Some(title) = self
                .main_stack
                .visible_child()
                .map(|c| self.main_stack.page(&c))
                .and_then(|p| p.title())
            {
                obj.announce(
                    &gettextrs::gettext("Switched to page {}").replace("{}", &title),
                    AccessibleAnnouncementPriority::Medium,
                );
            }

            if obj.breakpoint_collapsed() || !obj.sidebar_pinned() {
                self.split_view.set_show_sidebar(false);
            }
        }

        #[template_callback]
        fn navigation_popped(&self, _page: NavigationPage) {
            let obj = self.obj();

            if self.main_navigation.visible_page_tag().as_deref() == Some("root") {
                let is_feed_page = self.main_stack.visible_child_name() == Some("feed".into());
                obj.action_set_enabled("win.filter-enabled", is_feed_page);
                obj.action_set_enabled("win.filter-dialog", is_feed_page);
            }

            if let Some(title) = self.main_navigation.visible_page().map(|p| p.title()) {
                obj.announce(
                    &gettextrs::gettext("Switched to page {}").replace("{}", &title),
                    AccessibleAnnouncementPriority::Medium,
                );
            }
        }

        #[template_callback]
        fn copy_video_url(&self, video: Video) {
            let clipboard = WidgetExt::display(&*self.obj()).clipboard();
            clipboard.set_text(video.playable_url().as_ref());

            let toast = Toast::builder()
                .title(gettextrs::gettext("Copied URL"))
                .build();
            self.toast_overlay.add_toast(toast);
        }

        #[template_callback]
        fn remove_downloaded(&self, video: Video) {
            let obj = self.obj();
            let client = obj.client();

            if let Some(path) = video.downloaded_path() {
                client.remove_downloaded(&video);

                let path = Path::new(&path);

                if path.exists() {
                    if let Err(e) = std::fs::remove_file(path) {
                        obj.display_error(
                            gettextrs::gettext("Failed to remove downloaded file"),
                            Box::new(e),
                        );
                    } else {
                        let toast = Toast::builder()
                            .title(gettextrs::gettext("Removed downloaded copy of the video"))
                            .build();
                        self.toast_overlay.add_toast(toast);
                    }
                } else {
                    obj.show_toast(
                        Toast::builder()
                            .title(gettextrs::gettext(
                                "Downloaded video not available locally anymore",
                            ))
                            .build(),
                    );
                }
            }
        }

        // TODO: Potentially add a gsetting for that. Would not play too nicely with the Flatpak though.
        fn download_dir() -> Result<PathBuf, std::io::Error> {
            let mut videos = glib::user_special_dir(glib::enums::UserDirectory::Videos)
                .unwrap_or_else(|| {
                    let mut dir = glib::home_dir();
                    dir.push("Videos");
                    dir
                });
            videos.push("Pipeline");
            std::fs::create_dir_all(&videos)?;
            Ok(videos)
        }

        #[template_callback]
        fn download(&self, video: Video) {
            let obj = self.obj();
            gspawn!(clone!(
                #[weak]
                obj,
                async move {
                    let toast = Toast::builder()
                        .title(gettextrs::gettext("Started Download"))
                        .build();
                    obj.show_toast(toast);

                    let download_dir = Self::download_dir();

                    if let Err(e) = download_dir {
                        obj.display_error(
                            gettextrs::gettext(
                                "Error creating the directory where the video will be downloaded",
                            ),
                            Box::new(e) as Box<dyn std::error::Error + 'static>,
                        );
                        return;
                    }

                    let download_dir = download_dir.unwrap().into_os_string();
                    let download_format = obj.imp().settings.string("download-format");
                    let download_arguments = obj.imp().settings.string("download-yt-dlp-arguments");

                    let all_arguments = {
                        let mut v = vec![];
                        if config::FLATPAK {
                            v.push("--js-runtimes".into());
                            v.push("deno:/app/extensions/clapper/enhancers/bin/deno".into());
                        }
                        v.push("-P".into());
                        v.push(download_dir);
                        v.push("-f".into());
                        v.push(download_format.into());
                        v.extend(download_arguments.split(' ').map(|s| {
                            s.strip_prefix('"')
                                .unwrap_or(s)
                                .strip_suffix('"')
                                .unwrap_or(s)
                                .into()
                        }));
                        v
                    };

                    video.set_downloading(true);

                    let url = video.playable_url();
                    log::debug!("Downloading video {url} with arguments {all_arguments:?}");
                    let stream = tspawn!(async move { pytdlp::download(&url, all_arguments,) })
                        .await
                        .expect("Failed to spawn tokio");

                    if let Err(e) = stream {
                        obj.display_error(
                            gettextrs::gettext("Error downloading a video"),
                            Box::new(e) as Box<dyn std::error::Error + 'static>,
                        );
                        return;
                    }

                    let mut stream = stream.unwrap();

                    while let Some(result) = stream.next().await {
                        match result {
                            pytdlp::DownloadOutput::Log(l) => log::trace!("Download log: {}", l),
                            pytdlp::DownloadOutput::Progress(p) => {
                                log::trace!("Download progressed to {} percent", p.percent);
                                video.set_download_progress(p.percent / 100.0);
                            }
                            pytdlp::DownloadOutput::Finished(c) => {
                                obj.client().downloaded(video.clone(), c.filename);

                                let toast = Toast::builder()
                                    .title(gettextrs::gettext("Finished Download"))
                                    .build();
                                obj.show_toast(toast);
                            }
                            pytdlp::DownloadOutput::Error(e) => {
                                obj.display_error(
                                    gettextrs::gettext("Error downloading a video"),
                                    Box::new(e) as Box<dyn std::error::Error + 'static>,
                                );
                            }
                        }
                    }

                    video.set_downloading(false);
                }
            ));
        }

        fn set_breakpoint_collapsed(&self, value: bool) {
            let obj = self.obj();
            if !obj.is_fullscreen() {
                self.split_view.set_show_sidebar(!value);
                self.view_switcher.set_reveal(value);
            }
            self.breakpoint_collapsed.set(value);
        }

        fn compute_main_show_sidebar_visible(&self) {
            let obj = self.obj();
            obj.set_main_show_sidebar_visible(
                !self.split_view.shows_sidebar() && !self.view_switcher.reveals(),
            );
        }

        fn setup_compute_properties(&self) {
            let obj = self.obj();
            self.split_view.connect_show_sidebar_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.imp().compute_main_show_sidebar_visible();
                }
            ));
            self.view_switcher.connect_reveal_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.imp().compute_main_show_sidebar_visible();
                }
            ));
        }

        #[template_callback]
        fn handle_spinner_pressed(&self) {
            crate::pride::remove_seasonal_style(&self.progress_feed.get());
            self.progress_feed.add_css_class("rgb");
        }

        fn set_splash(&self) {
            use crate::splashes::*;
            let splash = Splash::random();
            self.label_splash.set_label(&splash.label());
        }

        fn setup_splashes(&self) {
            let obj = self.obj();

            self.settings
                .bind("show-splash", &self.label_splash.get(), "visible")
                .flags(SettingsBindFlags::DEFAULT)
                .build();

            self.overlay_videos.connect_visible_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.imp().set_splash();
                }
            ));
        }

        fn setup_sidebar(&self) {
            let stack = self.main_stack.clone();
            let list = self.sidebar_list.clone();

            let pages = stack.pages();
            let n_pages = pages.n_items();

            let mut page_names = Vec::with_capacity(n_pages.try_into().unwrap());

            let mut toggle_group: Option<gtk::ToggleButton> = None;

            for i in 0..n_pages {
                let page: ViewStackPage = pages.item(i).unwrap().downcast().unwrap();
                let page_name = page.name().expect("Page to have a name");
                page_names.push(page_name.clone());

                // TODO: Maybe put this in a blueprint file and in its own widget; code is pretty ugly.
                let icon = Image::builder()
                    .icon_name(
                        page.icon_name()
                            .expect("ViewStackPage to have an icon-name"),
                    )
                    .build();
                let label = Label::builder()
                    .label(page.title().expect("ViewStackPage to have a title"))
                    .build();
                let bx = gtk::Box::builder()
                    .spacing(12)
                    .orientation(Orientation::Horizontal)
                    .build();

                let toggle = gtk::ToggleButton::new();
                toggle.set_child(Some(&bx));
                toggle.add_css_class("flat");
                toggle.set_focusable(false);
                toggle.update_relation(&[Relation::LabelledBy(&[&label.clone().upcast()])]);

                if let Some(toggle_group) = toggle_group.as_ref() {
                    toggle.set_group(Some(toggle_group));
                } else {
                    toggle_group = Some(toggle.clone());
                }

                let row = ListBoxRow::builder().activatable(true).build();

                row.add_css_class("sidebar-item");

                bx.append(&icon);
                bx.append(&label);
                row.set_child(Some(&toggle));
                list.append(&row);

                toggle.connect_toggled(clone!(
                    #[weak]
                    stack,
                    #[strong]
                    page_name,
                    move |toggle| {
                        if toggle.is_active() {
                            stack.set_visible_child_name(&page_name);
                        }
                    }
                ));

                if i < MOBILE_BOTTOM_BAR_ITEMS {
                    self.obj()
                        .bind_property("breakpoint_collapsed", &row, "visible")
                        .sync_create()
                        .invert_boolean()
                        .build();
                }
            }

            list.connect_row_activated(move |_, row| {
                let toggle = row
                    .child()
                    .expect("ListBoxRow of sidebar to have a child")
                    .dynamic_cast::<gtk::ToggleButton>()
                    .expect("Child of sidebar to be a toggle");
                toggle.set_active(true);
            });

            stack.connect_visible_child_name_notify(clone!(
                #[strong]
                page_names,
                #[weak]
                list,
                move |stack| {
                    let name = stack
                        .visible_child_name()
                        .expect("Stack to have a visible child");
                    let index = page_names
                        .iter()
                        .position(|p| p == &name)
                        .expect("Visible child name of stack in page names");
                    let row = list
                        .row_at_index(index.try_into().unwrap())
                        .expect("Sidebar to have a row at bottom bar selected index");
                    row.activate();
                }
            ));

            // Select the first row on startup.
            toggle_group
                .expect("There is at least one elemnt in the sidebar")
                .set_active(true);
        }

        fn setup_bottom_bar(&self) {
            // Hide the view switcher entries of the bottom bar that are displayed in the sidebar.
            // Very ugly code.
            let view_switcher = self
                .view_switcher
                .first_child() // Action bar
                .and_then(|c| c.first_child()) // Revealer
                .and_then(|c| c.first_child()) // Center box
                .and_then(|c| c.first_child()) // Box
                .and_then(|c| c.next_sibling()); // View Switcher

            let mut child = view_switcher.and_then(|c| c.first_child());

            let mut i = 0;
            while let Some(widget) = child {
                if i >= MOBILE_BOTTOM_BAR_ITEMS {
                    widget.set_visible(false);
                }

                i += 1;
                child = widget.next_sibling();
            }
        }

        fn setup_main_navigation(&self) {
            self.main_navigation.push(&self.main_page.get());
        }

        fn setup_actions(&self, obj: &super::Window) {
            let action_settings = SimpleAction::new("settings", None);
            action_settings.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    let settings = PreferencesDialog::new(obj.client());
                    // Test hook: `KH_PREFS_PAGE=extensions` opens on that page.
                    #[cfg(feature = "dev-hooks")]
                    if let Ok(page) = std::env::var("KH_PREFS_PAGE") {
                        adw::prelude::PreferencesDialogExt::set_visible_page_name(&settings, &page);
                    }
                    settings.present(Some(&obj));
                }
            ));

            let action_toggle_filter = self.settings.create_action("filter-enabled");

            let action_filter_dialog = SimpleAction::new("filter-dialog", None);
            action_filter_dialog.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    let filter_dialog = FilterDialog::new(obj.client());
                    filter_dialog.present(Some(&obj));
                }
            ));

            let action_about = SimpleAction::new("about", None);
            action_about.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    let about_dialog = adw::AboutDialog::from_appdata(
                        &(config::RESOURCES_PATH.to_owned() + config::APP_ID + ".metainfo.xml"),
                        Some(env!("CARGO_PKG_VERSION")),
                    );

                    about_dialog.set_comments(concat!(
                        env!("CARGO_PKG_DESCRIPTION"),
                        "\n\nKinora is based on Pipeline by Julian Schmidhuber and contributors."
                    ));
                    about_dialog.add_credit_section(
                        Some("Pipeline"),
                        &["Julian Schmidhuber and contributors https://gitlab.com/schmiddi-on-mobile/pipeline"],
                    );

                    // translators: One per line: How you want to be credited as a translator, e.g. by the name you use, and optionally an email address ("Edgar Allan Poe <edgar@poe.com>")
                    about_dialog.set_translator_credits(&gettextrs::gettext("translator-credits"));
                    about_dialog.set_developers(
                        &(env!("CARGO_PKG_AUTHORS").split(':').collect::<Vec<&str>>()),
                    );
                    about_dialog.set_artists(&["David Lapshin <ddaudix@gmail.com>"]);
                    about_dialog.set_designers(&["Lo"]);
                    about_dialog.set_debug_info(&obj.debug_info());
                    about_dialog.present(Some(&obj));
                }
            ));

            // Kinora: connect or disconnect the YouTube account.
            let action_connect_account = SimpleAction::new("connect-account", None);
            action_connect_account.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    crate::gui::account_dialog::AccountDialog::new(&obj.client())
                        .present(Some(&obj));
                }
            ));

            let action_disconnect_account = SimpleAction::new("disconnect-account", None);
            action_disconnect_account.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    let settings = &obj.imp().settings;
                    let _ = settings.set_string("account-cookies-command", "");
                    let _ = settings.set_string("account-label", "");
                }
            ));

            let action_show_errors = SimpleAction::new("show-errors", None);
            action_show_errors.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    let dialog = ErrorDialog::new(&obj.imp().errors.borrow());
                    dialog.present(Some(&obj));
                }
            ));
            let action_clear_errors = SimpleAction::new("clear-errors", None);
            action_clear_errors.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.imp().errors.borrow_mut().clear();
                }
            ));

            let action_show_help_overlay = SimpleAction::new("show-help-overlay", None);
            action_show_help_overlay.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    let builder = Builder::from_resource("/ui/shortcuts.ui");
                    let shortcuts_window: ShortcutsDialog = builder
                        .object("help_overlay")
                        .expect("shortcuts.ui to have at least one object help_overlay");
                    shortcuts_window.present(Some(&obj));
                }
            ));

            let action_watch_later = SimpleAction::new("watch-later", None);
            action_watch_later.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    if let Some(video) = obj.imp().visible_video_page().and_then(|p| p.video()) {
                        obj.client().toggle_watch_later(video);
                    }
                }
            ));
            let action_subscribe = SimpleAction::new("subscribe", None);
            action_subscribe.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    if let Some(channel) =
                        obj.imp().visible_channel_page().and_then(|p| p.channel())
                    {
                        obj.client().toggle_subscription(channel);
                    }
                }
            ));
            let action_subscribe_or_watch_later =
                SimpleAction::new("subscribe-or-watch-later", None);
            action_subscribe_or_watch_later.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    // Just try both. In case e.g. the topmost stack element is not a channel, subscribing will do nothing.
                    let _ = WidgetExt::activate_action(&obj, "win.subscribe", None::<&Variant>);
                    let _ = WidgetExt::activate_action(&obj, "win.watch-later", None::<&Variant>);
                }
            ));

            let action_search = SimpleAction::new("search", None);
            action_search.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.imp().main_stack.set_visible_child_name("search");
                    // Explicitly notify that the sidebar child changed, otherwise the shortcut will not work in the search page.
                    obj.imp().sidebar_child_changed();
                }
            ));
            let action_reload = SimpleAction::new("reload", None);
            action_reload.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    if let Some(channel_page) = obj.imp().visible_channel_page() {
                        channel_page.reload();
                    } else if obj.imp().main_stack.visible_child_name().as_deref() == Some("home") {
                        let client = obj.client();
                        gspawn!(async move { client.refresh_home().await });
                    } else {
                        obj.imp().video_list.reload();
                    }
                }
            ));

            let action_tab_feed = SimpleAction::new("tab-feed", None);
            action_tab_feed.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.imp().main_stack.set_visible_child_name("feed");
                }
            ));
            let action_tab_watch_later = SimpleAction::new("tab-watch-later", None);
            action_tab_watch_later.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.imp().main_stack.set_visible_child_name("watch-later");
                }
            ));
            let action_tab_subscriptions = SimpleAction::new("tab-subscriptions", None);
            action_tab_subscriptions.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.imp().main_stack.set_visible_child_name("subscriptions");
                }
            ));

            let action_import = SimpleAction::new("import", None);
            action_import.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    let dialog = ImportDialog::new(obj.client());
                    dialog.present(Some(&obj));
                }
            ));
            // Kinora: subscribe to everything the connected YouTube account is subscribed to.
            let action_import_account = SimpleAction::new("import-account", None);
            action_import_account.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    let client = obj.client();
                    if client.importing_subscriptions() {
                        return;
                    }
                    gspawn!(clone!(
                        #[weak]
                        obj,
                        async move {
                            let message = match client.import_account_subscriptions().await {
                                Ok((0, total)) => gettextrs::ngettext(
                                    "Already subscribed to the {} channel",
                                    "Already subscribed to all {} channels",
                                    total as u32,
                                )
                                .replace("{}", &total.to_string()),
                                Ok((added, _)) => gettextrs::ngettext(
                                    "Imported {} channel",
                                    "Imported {} channels",
                                    added as u32,
                                )
                                .replace("{}", &added.to_string()),
                                Err(e) => gettextrs::gettext("Could not import subscriptions: {}")
                                    .replace("{}", &e),
                            };
                            obj.show_toast(Toast::new(&message));
                            // Show the new channels' videos in the feed.
                            obj.imp().video_list.reload();
                        }
                    ));
                }
            ));
            let action_export = SimpleAction::new("export", None);
            action_export.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    let dialog = ExportDialog::new(obj.client());
                    dialog.present(Some(&obj));
                }
            ));
            let action_toggle_sidebar = SimpleAction::new("toggle-sidebar", None);
            action_toggle_sidebar.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    let imp = obj.imp();
                    imp.split_view
                        .set_show_sidebar(!imp.split_view.shows_sidebar());
                }
            ));
            let action_privacy_notice = SimpleAction::new("privacy-notice", None);
            action_privacy_notice.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.imp().show_privacy_notice();
                }
            ));

            let actions = SimpleActionGroup::new();
            obj.insert_action_group("win", Some(&actions));

            // Kinora: the player sheet (see docs/design.md, "Player").
            let player_actions: [(&str, fn(&Self)); 11] = [
                // "How do you want to watch?" (see `gui/first_run.rs`).
                ("ways-to-watch", |s| {
                    crate::gui::first_run::present(&s.obj(), &s.settings)
                }),
                // The DJ (see `gui/dj.rs`).
                ("dj-start", |s| s.dj().start(false)),
                ("dj-start-mix", |s| s.dj().start(true)),
                ("dj-skip", |s| s.dj().skip()),
                ("dj-leave", |s| s.dj().leave()),
                ("minimize-player", |s| s.player_sheet.minimize()),
                ("expand-player", |s| s.player_sheet.expand()),
                ("toggle-player", |s| s.player_sheet.toggle()),
                ("close-player", |s| s.player_sheet.close()),
                ("player-next", |s| s.player_skip(true)),
                ("player-previous", |s| s.player_skip(false)),
            ];
            for (name, activate) in player_actions {
                let action = SimpleAction::new(name, None);
                action.connect_activate(clone!(
                    #[weak]
                    obj,
                    move |_, _| activate(obj.imp())
                ));
                actions.add_action(&action);
            }
            // Test hooks for the DJ: what the session holds (to the log), and
            // the queue's buttons applied to its third track.
            #[cfg(feature = "dev-hooks")]
            {
                let dev_actions: [(&str, fn(&Self)); 6] = [
                    ("dj-show-queue", |s| {
                        if let Some(page) = s.current_video_page() {
                            page.dev_scroll_to(430.0);
                        }
                    }),
                    ("dj-near-end", |s| {
                        if let Some(page) = s.current_video_page() {
                            page.dev_seek_near_end(14.0);
                        }
                    }),
                    ("dj-log", |s| s.dj().dev_log()),
                    ("dj-queue-play-next", |s| s.dj().dev_queue("play-next")),
                    ("dj-queue-remove", |s| s.dj().dev_queue("remove")),
                    ("dj-queue-play-now", |s| s.dj().dev_queue("play-now")),
                ];
                for (name, activate) in dev_actions {
                    let action = SimpleAction::new(name, None);
                    action.connect_activate(clone!(
                        #[weak]
                        obj,
                        move |_, _| activate(obj.imp())
                    ));
                    actions.add_action(&action);
                }
            }
            // A playlist or mix card was opened: the DJ plays it (id, title).
            let action_dj_playlist = SimpleAction::new(
                "dj-playlist",
                Some(&<(String, String)>::static_variant_type()),
            );
            action_dj_playlist.connect_activate(clone!(
                #[weak]
                obj,
                move |_, target| {
                    if let Some((id, title)) = target.and_then(|t| t.get::<(String, String)>()) {
                        obj.imp().dj().start_playlist(&id, &title);
                    }
                }
            ));
            actions.add_action(&action_dj_playlist);
            actions.add_action(&action_settings);
            actions.add_action(&action_toggle_filter);
            actions.add_action(&action_filter_dialog);
            actions.add_action(&action_show_help_overlay);
            actions.add_action(&action_about);
            actions.add_action(&action_connect_account);
            actions.add_action(&action_disconnect_account);
            actions.add_action(&action_show_errors);
            actions.add_action(&action_clear_errors);
            actions.add_action(&action_watch_later);
            actions.add_action(&action_subscribe);
            actions.add_action(&action_subscribe_or_watch_later);
            actions.add_action(&action_search);
            actions.add_action(&action_reload);
            actions.add_action(&action_tab_feed);
            actions.add_action(&action_tab_watch_later);
            actions.add_action(&action_tab_subscriptions);
            actions.add_action(&action_import);
            actions.add_action(&action_import_account);
            actions.add_action(&action_export);
            actions.add_action(&action_toggle_sidebar);
            actions.add_action(&action_privacy_notice);
        }

        fn setup_client(&self) {
            let obj = self.obj();
            let client = obj.client();
            let res = client.setup();

            if let Err(e) = res {
                log::error!("Error setting up the client: {e}");
                obj.display_error(gettextrs::gettext("Failed to set up application"), e);
            }

            gspawn!(clone!(
                #[weak(rename_to = s)]
                self,
                async move {
                    s.obj().client().setup_async().await;
                    if s.settings.boolean("reload-on-startup") {
                        s.video_list.reload();
                    }
                    // Kinora: load the account's home feed.
                    s.obj().client().refresh_home().await;
                    s.first_run();
                    #[cfg(feature = "dev-hooks")]
                    s.dev_hooks();
                }
            ));
        }

        /// Kinora: "How do you want to watch?", once (see `gui/first_run.rs`).
        /// A test session (any `KH_` hook set) is not interrupted by it;
        /// `KH_FIRST_RUN=1` shows it there.
        fn first_run(&self) {
            let forced = cfg!(feature = "dev-hooks") && std::env::var("KH_FIRST_RUN").is_ok();
            let testing =
                cfg!(feature = "dev-hooks") && std::env::vars().any(|(k, _)| k.starts_with("KH_"));
            if forced || (!testing && crate::gui::first_run::is_due(&self.settings)) {
                crate::gui::first_run::present(&self.obj(), &self.settings);
            }
        }

        fn show_privacy_notice(&self) {
            let obj = self.obj();
            let privacy_notice = PrivacyNotice::new();
            privacy_notice.present(Some(&*obj));
        }

        fn show_privacy_toast(&self) {
            let obj = self.obj();
            let toast = Toast::builder()
                .title(gettextrs::gettext("Privacy Notice"))
                .button_label(gettextrs::gettext("More Information"))
                .build();
            toast.connect_button_clicked(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.imp().show_privacy_notice();
                }
            ));

            obj.show_toast(toast);
            let _ = self.settings.set_boolean("setup-window-shown", true);
        }

        fn setup_filter_icon(&self) {
            let obj = self.obj();
            self.settings.connect_changed(
                Some("filter-enabled"),
                clone!(
                    #[weak]
                    obj,
                    move |_, _| {
                        obj.imp().set_filter_icon();
                    }
                ),
            );
            self.set_filter_icon();
        }

        fn set_filter_icon(&self) {
            let enabled = self.settings.boolean("filter-enabled");
            let icon_name = if enabled {
                "funnel-symbolic"
            } else {
                "funnel-outline-symbolic"
            };
            self.btn_filter_menu.set_icon_name(icon_name);
        }

        fn setup_fullscreen(&self) {
            // Unfullscreen the video page if the window gets unfullscreened.
            let obj = self.obj();
            obj.connect_fullscreened_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    let Some(video_page) = obj.imp().visible_video_page() else {
                        return;
                    };
                    if !obj.is_fullscreen()
                        && video_page.fullscreened()
                        && !video_page.window_mode()
                    {
                        video_page.set_fullscreened(false);
                    }
                }
            ));
        }

        /// Kinora: the mini-player sits above the narrow window's bottom bar.
        fn setup_player_sheet(&self) {
            let obj = self.obj();
            let update = clone!(
                #[weak]
                obj,
                move || {
                    let imp = obj.imp();
                    let bar = &imp.view_switcher;
                    let inset = if bar.reveals() {
                        bar.height()
                            .max(bar.measure(gtk::Orientation::Vertical, -1).1)
                    } else {
                        0
                    };
                    imp.player_sheet.set_bottom_inset(inset);
                }
            );
            self.view_switcher.connect_reveal_notify(move |_| {
                update();
                // Again once the bar has finished sliding in or out.
                glib::timeout_add_local_once(std::time::Duration::from_millis(350), update.clone());
            });
        }

        /// Kinora: the DJ, and whether it is offered (its extension is on).
        fn setup_dj(&self) {
            let obj = self.obj();
            let _ = self.dj.set(crate::gui::dj::Dj::new(&obj));
            let update = clone!(
                #[weak]
                obj,
                move |settings: &gtk::gio::Settings| {
                    let off = settings
                        .strv("disabled-extensions")
                        .iter()
                        .any(|id| id.as_str() == kinora_core::extension::builtin::dj::ID);
                    obj.set_dj_available(!off);
                    if off {
                        obj.imp().dj().leave();
                    }
                }
            );
            update(&self.settings);
            self.settings
                .connect_changed(Some("disabled-extensions"), move |settings, _| {
                    update(settings)
                });
        }

        fn allow_all_clapper_enhancers(&self) {
            for proxy in clapper::global_enhancer_proxies()
                .iter()
                .flat_map(|p| (0..p.n_proxies()).flat_map(|i| p.peek_proxy(i)))
            {
                proxy.set_target_creation_allowed(true);
            }
        }

        fn apply_seasonal_style(&self) {
            if self.settings.boolean("seasonal-style") {
                crate::pride::apply_seasonal_style(&self.progress_feed.get());
            } else {
                crate::pride::remove_seasonal_style(&self.progress_feed.get());
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "TFWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            VideoItem::ensure_type();
            crate::gui::home_view::HomeView::ensure_type();
            VideoPage::ensure_type();
            ChannelItem::ensure_type();
            VideoList::ensure_type();
            FiniteVideoList::ensure_type();
            SubscriptionsList::ensure_type();
            VideoFetcherStream::ensure_type();
            SubscriptionsStore::ensure_type();
            SearchList::ensure_type();
            SearchStream::ensure_type();
            PlayerSheet::ensure_type();
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
            Utility::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup_actions(&self.obj());
            self.setup_splashes();
            self.setup_sidebar();
            self.setup_bottom_bar();
            self.setup_main_navigation();
            self.setup_compute_properties();
            self.setup_filter_icon();
            self.setup_search_source();
            self.setup_fullscreen();
            self.setup_player_sheet();
            self.setup_dj();
            self.allow_all_clapper_enhancers();

            self.setup_client();
            self.sidebar_child_changed();

            let obj = self.obj();
            if PROFILE == "Devel" {
                obj.add_css_class("devel");
            }
            obj.load_window_size();

            // Kinora: "Lite" switches the effects off (see docs/design.md).
            let apply_reduce_effects = |window: &super::Window, settings: &gdk::gio::Settings| {
                if settings.boolean("reduce-effects") {
                    window.add_css_class("reduce-effects");
                } else {
                    window.remove_css_class("reduce-effects");
                }
            };
            apply_reduce_effects(&obj, &self.settings);
            self.settings.connect_changed(
                Some("reduce-effects"),
                clone!(
                    #[weak]
                    obj,
                    move |settings, _| apply_reduce_effects(&obj, settings)
                ),
            );

            self.apply_seasonal_style();
            self.settings.connect_changed(
                Some("seasonal-style"),
                clone!(
                    #[strong]
                    obj,
                    move |_, _| {
                        obj.imp().apply_seasonal_style();
                    }
                ),
            );

            self.settings
                .bind("watch-history-enabled", &*obj, "watch-history-enabled")
                .build();

            self.settings
                .bind("sidebar-pinned", &*obj, "sidebar-pinned")
                .build();

            if !self.settings.boolean("setup-window-shown") {
                self.show_privacy_toast();
            }
        }
    }

    impl WidgetImpl for Window {}
    impl WindowImpl for Window {
        fn close_request(&self) -> Propagation {
            let obj = self.obj();
            if let Err(err) = obj.save_window_size() {
                log::warn!("Failed to save window state, {}", err);
            }

            self.main_navigation.pop_to_tag("root");
            // Kinora: the video in the player counts as watched.
            if let Some(video) = self.visible_video_page().and_then(|p| p.video()) {
                obj.client().watched(video, Utc::now());
            }

            let _ = obj.client().cleanup();

            self.parent_close_request()
        }
    }
    impl ApplicationWindowImpl for Window {}
    impl AdwWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

/// Test hooks: the first video page in the window (the player sheet's).
#[cfg(feature = "dev-hooks")]
fn dev_find_video_page(root: &gtk::Widget) -> Option<gtk::Widget> {
    use gtk::prelude::*;
    if root.is::<crate::gui::video_page::VideoPage>() {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = dev_find_video_page(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}
