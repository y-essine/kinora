use std::{borrow::Borrow, path::Path};

use adw::Toast;
use clapper::MediaItem;
use clapper_gtk::prelude::AvExt;
use gdk::prelude::SettingsExt;
use glib::{Object, clone, object::Cast, subclass::types::ObjectSubclassIsExt};
use gtk::prelude::WidgetExt;
use url::Url;

use crate::{
    backend::{Client, Video},
    gspawn,
    gui::utility::Utility,
};

use super::window::Window;

gtk::glib::wrapper! {
    pub struct VideoPage(ObjectSubclass<imp::VideoPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

impl VideoPage {
    pub fn new(video: &Video, client: &Client) -> Self {
        Object::builder()
            .property("client", client)
            .property("video", video)
            .build()
    }

    pub fn play(&self) {
        self.set_external(false);
        if let Some(video) = self.video() {
            let imp = self.imp();
            // Play with internal clapper
            let player = imp
                .clapper
                .player()
                .expect("Clapper does not have a player");
            let queue = player.queue().expect("Clapper does not have a queue");
            queue.clear();
            queue.add_item(&MediaItem::new(
                &self.convert_to_clapper_playable_url(&video),
            ));
            player.play();
            self.imp().play_started.set(Some(std::time::Instant::now()));
            // Kinora: count it on the YouTube account once it was really watched.
            self.mark_watched_on_account_later(&video);
        } else {
            self.pause();
        }
    }

    pub fn play_pause(&self) {
        let player = self
            .imp()
            .clapper
            .player()
            .expect("Clapper does not have a player");
        match player.state() {
            clapper::PlayerState::Paused => player.play(),
            clapper::PlayerState::Playing => player.pause(),
            _ => {}
        }
    }

    fn seek_by(&self, dt: f64) {
        let player = self.imp().clapper.player();
        if let Some(player) = player
            && let Some(duration) = player
                .queue()
                .and_then(|q| q.current_item())
                .map(|i| i.duration())
        {
            let mut pending_position = player.position() + dt;
            if pending_position < 0.0 {
                pending_position = 0.0;
            } else if pending_position > duration {
                pending_position = duration;
            }
            player.seek(pending_position);
        }
    }

    fn seek_to_percent(&self, percent: f64) {
        let player = self.imp().clapper.player();
        if let Some(player) = player
            && let Some(duration) = player
                .queue()
                .and_then(|q| q.current_item())
                .map(|i| i.duration())
        {
            let pending_position = percent * duration;
            player.seek(pending_position);
        }
    }

    pub fn play_external(&self) {
        self.pause();
        self.set_external(true);

        if let Some(video) = self.video() {
            let imp = self.imp();
            // Kinora: count it on the YouTube account.
            self.client().mark_watched_on_account(&video);

            let player = imp.settings.string("player");

            if player.is_empty() {
                self.window().show_toast(
                    Toast::builder()
                        .title(gettextrs::gettext(
                            "Configure your external player in the preferences first",
                        ))
                        .build(),
                );
                return;
            }

            let player_is_clapper = player
                .split_whitespace()
                .any(|w| w == "clapper" || w == "com.github.rafostar.Clapper");
            let url = if player_is_clapper {
                self.convert_to_clapper_playable_url(&video)
            } else {
                self.downloaded_path_or_url(&video)
            };
            let remove_gsk_renderer_env_variable = imp.settings.boolean("cairo-software-renderer");

            gspawn!(clone!(
                #[weak(rename_to = s)]
                self,
                async move {
                    let res =
                        Utility::run_command_on_url(player, url, remove_gsk_renderer_env_variable)
                            .await;
                    if let Err(e) = res {
                        s.window()
                            .display_error(gettextrs::gettext("Failed to spawn video player"), e);
                    }
                }
            ));
        }
    }

    pub fn pause(&self) {
        // Stop playing.
        let player = self
            .imp()
            .clapper
            .player()
            .expect("Clapper does not have a player");
        player.pause();
    }

    /// Kinora: stop for good (another video replaces this page): pause and
    /// unload the video, so it no longer plays or streams.
    pub fn stop(&self) {
        let player = self
            .imp()
            .clapper
            .player()
            .expect("Clapper does not have a player");
        // Only pause: the page is destroyed right after, which ends the player.
        // Stopping or clearing the queue here raced with Clapper's player thread
        // (a crash, or the next page's player never starting).
        player.pause();
    }

    /// Kinora: mark the video watched on the YouTube account after 30 seconds
    /// of it still being on this page. Doing it at once also competed with the
    /// player for the network and CPU right when the video was starting.
    fn mark_watched_on_account_later(&self, video: &Video) {
        let page = glib::prelude::ObjectExt::downgrade(self);
        let video = video.clone();
        glib::timeout_add_seconds_local_once(30, move || {
            if let Some(page) = page.upgrade() {
                if page.video().as_ref() == Some(&video) {
                    page.client().mark_watched_on_account(&video);
                }
            }
        });
    }

    pub fn finished(&self) -> bool {
        if self.external() {
            return true;
        }

        let player = self
            .imp()
            .clapper
            .player()
            .expect("Clapper does not have a player");

        if let Some(duration) = player
            .queue()
            .and_then(|q| q.current_item())
            .map(|i| i.duration())
        {
            // Kinora: the player can stop a hair past the duration.
            player.position() >= duration - 0.75
        } else {
            true
        }
    }

    fn window(&self) -> Window {
        self.root().unwrap().dynamic_cast().unwrap()
    }

    fn try_get_downloaded(&self, video: &Video) -> Option<String> {
        if let Some(path) = video.downloaded_path() {
            if Path::new(&path).exists() {
                Some(path.clone())
            } else {
                self.client().remove_downloaded(video);
                video.set_downloaded_path(None::<String>);
                self.window().show_toast(
                    Toast::builder()
                        .title(gettextrs::gettext(
                            "Downloaded video not available locally anymore",
                        ))
                        .build(),
                );
                None
            }
        } else {
            None
        }
    }

    fn downloaded_path_or_url(&self, video: &Video) -> String {
        if let Some(downloaded) = self.try_get_downloaded(video) {
            downloaded
        } else {
            video.playable_url().to_string()
        }
    }

    fn convert_to_clapper_playable_url(&self, video: &Video) -> String {
        if let Some(downloaded) = self
            .try_get_downloaded(video)
            .and_then(|d| Url::from_file_path(d).ok())
        {
            return downloaded.to_string();
        }

        let url = video.playable_url().to_string();
        match video.id().platform.borrow() {
            "peertube" => {
                // This would be better expressed as `Url::set_scheme`, but this would error as `http(s)` is "special" and `peertube` is not.
                if let Some((_, rest)) = url.split_once("://") {
                    format!("peertube://{rest}")
                } else {
                    url
                }
            }
            _ => url,
        }
    }
}

pub mod imp {
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::marker::PhantomData;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use adw::subclass::bin::BinImpl;
    use clapper_gtk::prelude::AvExt;
    use gdk::gio::SimpleAction;
    use gdk::gio::SimpleActionGroup;
    use glib::Propagation;
    use glib::Properties;
    use glib::clone;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::CallbackAction;
    use gtk::CompositeTemplate;
    use gtk::Shortcut;
    use gtk::ShortcutController;
    use gtk::ShortcutTrigger;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    use crate::backend::Channel;
    use crate::backend::Client;
    use crate::backend::FiniteVideoStore;
    use crate::backend::PlayType;
    use crate::backend::Video;
    use crate::backend::VideoSort;
    use crate::config::APP_ID;
    use crate::config::BASE_ID;
    use crate::gspawn;
    use crate::gui::utility::Utility;
    use crate::gui::video_item::VideoItem;

    static INSTANCE_COUNT: AtomicUsize = AtomicUsize::new(0);

    #[derive(CompositeTemplate, Properties)]
    #[template(resource = "/ui/video_page.ui")]
    #[properties(wrapper_type = super::VideoPage)]
    pub struct VideoPage {
        #[template_child]
        pub(super) clapper: TemplateChild<clapper_gtk::Video>,
        #[property(get, set = Self::set_video, nullable)]
        video: RefCell<Option<Video>>,
        #[property(get = Self::get_state, builder(clapper::PlayerState::Stopped))]
        state: PhantomData<clapper::PlayerState>,
        #[property(get = Self::get_duration)]
        duration: PhantomData<f64>,
        #[property(get = Self::get_position)]
        position: PhantomData<f64>,

        #[template_child]
        billboard: TemplateChild<clapper_gtk::Billboard>,

        // Kinora: "Up next"
        #[template_child]
        up_next_box: TemplateChild<gtk::Box>,
        #[template_child]
        up_next_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        autoplay_switch: TemplateChild<gtk::Switch>,
        up_next_store: FiniteVideoStore,
        autoplay_done: Cell<bool>,
        /// When play() was pressed, to log how long the video took to start.
        pub(super) play_started: Cell<Option<std::time::Instant>>,

        #[property(get, set)]
        fullscreened: Cell<bool>,
        /// Kinora: while fullscreened, fill the window instead of the screen.
        #[property(get, set)]
        window_mode: Cell<bool>,
        #[property(get, set)]
        show_sidebar_visible: Cell<bool>,
        #[property(get, set)]
        primary_menu_visible: Cell<bool>,

        #[property(get, set)]
        external: Cell<bool>,

        #[property(get, set)]
        client: RefCell<Client>,

        watch: RefCell<Option<gtk::ExpressionWatch>>,
        pub settings: gtk::gio::Settings,
    }

    impl Default for VideoPage {
        fn default() -> Self {
            Self {
                clapper: Default::default(),
                video: Default::default(),
                billboard: Default::default(),
                up_next_box: Default::default(),
                up_next_list: Default::default(),
                autoplay_switch: Default::default(),
                up_next_store: FiniteVideoStore::with_sort(VideoSort::AsAdded),
                autoplay_done: Default::default(),
                play_started: Default::default(),
                fullscreened: Default::default(),
                window_mode: Default::default(),
                state: Default::default(),
                position: Default::default(),
                duration: Default::default(),
                show_sidebar_visible: Default::default(),
                primary_menu_visible: Default::default(),
                external: Default::default(),
                client: Default::default(),
                watch: Default::default(),
                settings: gtk::gio::Settings::new(BASE_ID),
            }
        }
    }

    impl VideoPage {
        // Recommended syntax is ugly and the item will be moved to `pedantic` soon.
        #[allow(clippy::assigning_clones)]
        fn set_video(&self, video: Option<Video>) {
            *self.video.borrow_mut() = video.clone();

            if let Some(video) = video {
                let obj = self.obj();
                let client = obj.client();

                // Kinora: load "Up next" alongside the video information.
                self.autoplay_done.set(false);
                self.up_next_store.clear();
                self.up_next_box.set_visible(false);
                gspawn!(clone!(
                    #[weak]
                    obj,
                    #[weak]
                    client,
                    #[weak]
                    video,
                    async move {
                        let videos = client.up_next(&video).await;
                        let imp = obj.imp();
                        // The page may have moved on to another video meanwhile.
                        if obj.video().as_ref() != Some(&video) {
                            return;
                        }
                        imp.up_next_store.clear();
                        for v in videos {
                            imp.up_next_store.add(v);
                        }
                        imp.up_next_box.set_visible(imp.up_next_store.n_items() > 0);
                    }
                ));

                // Load additional data
                gspawn!(
                    clone!(
                        #[weak]
                        obj,
                        #[weak]
                        client,
                        #[weak]
                        video,
                        async move {
                            let result = client.populate_video_information(&video.id()).await;
                            if let Err(e) = result {
                                log::error!("Failed to populate video information: {e}");
                                obj.window().display_error(
                                    gettextrs::gettext("Failed to populate video information"),
                                    Box::new(e),
                                );
                            }

                            // The video thumbnail could have changed after updating the information; reload the thumbnail.
                            client.populate_thumbnail(&video).await;

                            // Kinora: an "Up next" video can still be waiting for its
                            // channel; there is nothing to look up yet.
                            let channel_id = video.uploader().id();
                            let result = if channel_id.id.is_empty() {
                                Ok(())
                            } else {
                                client.populate_channel_information(&channel_id).await
                            };
                            if let Err(e) = result {
                                log::error!("Failed to populate channel information: {e}");
                                obj.window().display_error(
                                    gettextrs::gettext("Failed to fetch channel information"),
                                    Box::new(e),
                                );
                            }
                        },
                    ),
                    glib::Priority::LOW
                );
            }
        }

        /// Kinora: toggle screen (`window_mode == false`) or window fullscreen.
        /// Pressing the other mode's button switches straight over.
        pub(super) fn toggle_fullscreen_mode(&self, window_mode: bool) {
            let obj = self.obj();
            if obj.fullscreened() {
                let same_mode = obj.window_mode() == window_mode;
                obj.set_fullscreened(false);
                if same_mode {
                    return;
                }
            }
            obj.set_window_mode(window_mode);
            obj.set_fullscreened(true);
        }

        /// When the video has played to its end, continue with the first "Up next" video.
        fn maybe_autoplay(&self, player: &clapper::Player) {
            let obj = self.obj();
            if player.state() == clapper::PlayerState::Playing {
                if let Some(started) = self.play_started.take() {
                    log::info!(
                        "Player: playing after {:.1} s",
                        started.elapsed().as_secs_f64()
                    );
                }
            }
            log::debug!(
                "autoplay check: state {:?}, position {:.2}, duration {:?}, up next {}",
                player.state(),
                player.position(),
                player
                    .queue()
                    .and_then(|q| q.current_item())
                    .map(|i| i.duration()),
                self.up_next_store.n_items()
            );
            // Live streams have no real end; their position runs with the duration.
            if self.autoplay_done.get()
                || obj.video().is_some_and(|v| v.is_live())
                || obj.external()
                || !self.settings.boolean("autoplay-next")
                || !matches!(
                    player.state(),
                    clapper::PlayerState::Stopped | clapper::PlayerState::Paused
                )
            {
                return;
            }
            let duration = player
                .queue()
                .and_then(|q| q.current_item())
                .map(|i| i.duration())
                .unwrap_or(0.0);
            // Only at the very end, not on an ordinary pause.
            if duration <= 0.0 || player.position() < duration - 0.75 {
                return;
            }
            let Some(next) = self
                .up_next_store
                .item(0)
                .and_then(|o| o.downcast::<Video>().ok())
            else {
                return;
            };
            self.autoplay_done.set(true);
            obj.emit_by_name::<()>("play-video", &[&next, &PlayType::Default]);
        }

        fn get_state(&self) -> clapper::PlayerState {
            self.clapper
                .player()
                .expect("Clapper to have a player")
                .state()
        }

        fn get_position(&self) -> f64 {
            self.clapper
                .player()
                .expect("Clapper to have a player")
                .position()
        }

        fn get_duration(&self) -> f64 {
            self.clapper
                .player()
                .expect("Clapper to have a player")
                .queue()
                .expect("Player to have a queue")
                .current_item()
                .expect("Queue to have a current item")
                .duration()
        }
    }

    #[gtk::template_callbacks]
    impl VideoPage {
        #[template_callback(function)]
        fn format_uploaded(uploaded: String) -> String {
            // Kinora: nothing until the upload date is known.
            if uploaded.is_empty() {
                return String::new();
            }
            gettextrs::gettext("Uploaded {}").replace("{}", &uploaded)
        }

        #[template_callback]
        fn up_next_row_activated(&self, row: &gtk::ListBoxRow) {
            if let Some(video) = self
                .up_next_store
                .item(row.index() as u32)
                .and_then(|o| o.downcast::<Video>().ok())
            {
                self.obj()
                    .emit_by_name::<()>("play-video", &[&video, &PlayType::Default]);
            }
        }

        #[template_callback]
        fn display_channel(&self) {
            let obj = self.obj();
            if let Some(channel) = obj.video().map(|v| v.uploader()) {
                obj.emit_by_name::<()>("display-channel", &[&channel]);
            }
        }

        #[template_callback]
        fn toggle_fullscreen(&self) {
            self.toggle_fullscreen_mode(false);
        }

        /// Kinora: the video fills the window, not the screen.
        #[template_callback]
        fn toggle_window_fullscreen(&self) {
            self.toggle_fullscreen_mode(true);
        }

        #[template_callback(function)]
        fn window_fullscreen_icon(fullscreened: bool, window_mode: bool) -> &'static str {
            if fullscreened && window_mode {
                "window-restore-symbolic"
            } else {
                "window-maximize-symbolic"
            }
        }

        #[template_callback(function)]
        fn window_fullscreen_tooltip(fullscreened: bool, window_mode: bool) -> String {
            if fullscreened && window_mode {
                gettextrs::gettext("Exit Window Fullscreen (T)")
            } else {
                gettextrs::gettext("Window Fullscreen (T)")
            }
        }

        #[template_callback]
        fn download(&self) {
            let obj = self.obj();
            if let Some(video) = obj.video() {
                obj.emit_by_name::<()>("download-video", &[&video]);
            }
        }

        #[template_callback]
        fn remove_downloaded(&self) {
            let obj = self.obj();
            if let Some(video) = obj.video() {
                obj.emit_by_name::<()>("remove-downloaded", &[&video]);
            }
        }

        #[template_callback]
        fn play_external(&self) {
            let obj = self.obj();
            obj.play_external();
        }

        #[template_callback]
        fn copy_to_clipboard(&self) {
            let obj = self.obj();
            if let Some(video) = obj.video() {
                obj.emit_by_name::<()>("copy-video-url-to-clipboard", &[&video]);
            }
        }

        fn setup_actions(&self) {
            let obj = self.obj();

            let action_play_pause = SimpleAction::new("play-pause", None);
            action_play_pause.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.play_pause();
                }
            ));
            let action_seek_forward = SimpleAction::new("seek-forward", None);
            action_seek_forward.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    // TODO: Maybe configurable duration.
                    obj.seek_by(10.0);
                    obj.imp()
                        .billboard
                        .post_message("media-seek-forward", &gettextrs::gettext("10 Seconds"));
                }
            ));

            let action_seek_backward = SimpleAction::new("seek-backward", None);
            action_seek_backward.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    // TODO: Maybe configurable duration.
                    obj.seek_by(-10.0);
                    obj.imp()
                        .billboard
                        .post_message("media-seek-backward", &gettextrs::gettext("10 Seconds"));
                }
            ));

            let action_seek_percentile =
                SimpleAction::new("seek-percentile", Some(&i32::static_variant_type()));
            action_seek_percentile.connect_activate(clone!(
                #[weak]
                obj,
                move |_, parameter| {
                    let parameter: i32 = parameter.unwrap().get().unwrap();
                    obj.seek_to_percent((parameter as f64) / 10.0)
                }
            ));

            self.clapper.connect_seek_request(clone!(
                #[weak]
                obj,
                move |_, forward| {
                    // TODO: Maybe configurable duration.
                    if forward {
                        obj.seek_by(10.);
                        obj.imp()
                            .billboard
                            .post_message("media-seek-forward", &gettextrs::gettext("10 Seconds"));
                    } else {
                        obj.seek_by(-10.);
                        obj.imp()
                            .billboard
                            .post_message("media-seek-backward", &gettextrs::gettext("10 Seconds"));
                    }
                }
            ));

            let actions = SimpleActionGroup::new();
            obj.insert_action_group("video", Some(&actions));
            actions.add_action(&action_play_pause);
            actions.add_action(&action_seek_forward);
            actions.add_action(&action_seek_backward);
            actions.add_action(&action_seek_percentile);

            let clapper_actions = [
                "speed-up",
                "speed-down",
                "volume-up",
                "volume-down",
                "toggle-mute",
            ];
            let clapper = self.clapper.get();
            for action_name in clapper_actions {
                let action = SimpleAction::new(action_name, None);
                action.connect_activate(clone!(
                    #[weak]
                    clapper,
                    move |_, _| {
                        let _ = clapper.activate_action(&format!("video.{action_name}"), None);
                    }
                ));
                actions.add_action(&action);
            }
        }

        fn setup(&self) {
            let obj = self.obj();
            let settings = &self.settings;
            let billboard = &self.billboard;

            let shortcut_escape = Shortcut::new(
                Some(
                    ShortcutTrigger::parse_string("Escape")
                        .expect("Escape to be a valid shortcut trigger"),
                ),
                Some(CallbackAction::new(clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_panic]
                    move |_, _| {
                        if obj.fullscreened() {
                            obj.set_fullscreened(false);
                            Propagation::Stop
                        } else {
                            Propagation::Proceed
                        }
                    }
                ))),
            );
            let shortcut_fullscreen = Shortcut::new(
                Some(
                    ShortcutTrigger::parse_string("F|F11")
                        .expect("F|F11 to be a valid shortcut trigger"),
                ),
                Some(CallbackAction::new(clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_panic]
                    move |_, _| {
                        obj.imp().toggle_fullscreen();
                        Propagation::Stop
                    }
                ))),
            );

            // Kinora: "T" fills the window, like YouTube's theater mode.
            let shortcut_window_fullscreen = Shortcut::new(
                Some(ShortcutTrigger::parse_string("T").expect("T to be a valid shortcut trigger")),
                Some(CallbackAction::new(clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_panic]
                    move |_, _| {
                        obj.imp().toggle_fullscreen_mode(true);
                        Propagation::Stop
                    }
                ))),
            );

            let shortcut_controller = ShortcutController::new();
            shortcut_controller.add_shortcut(shortcut_escape);
            shortcut_controller.add_shortcut(shortcut_fullscreen);
            shortcut_controller.add_shortcut(shortcut_window_fullscreen);
            obj.add_controller(shortcut_controller);

            // Set up MPRIS
            // Make sure that every instance of VideoPage gets its own own_name, otherwise having multiple videos opened at the same time (e.g. one in the bottom bar and one in the navigation stack) would overwrite each other.
            let player = self.clapper.player().expect("Clapper to have a Player");
            let proxies = player.enhancer_proxies();
            let mpris = proxies.and_then(|p| p.proxy_by_module("clapper-mpris"));

            if let Some(mpris) = mpris {
                mpris.set_locally(HashMap::from([
                    ("app-id", &APP_ID.to_value()),
                    (
                        "own-name",
                        &format!(
                            "org.mpris.MediaPlayer2.kinora.instance{}",
                            INSTANCE_COUNT.fetch_add(1, Ordering::Relaxed)
                        )
                        .to_value(),
                    ),
                    ("identity", &gettextrs::gettext("Kinora").to_value()),
                    ("desktop-entry", &APP_ID.to_value()),
                ]));
                mpris.set_target_creation_allowed(true);
            }

            player.connect_state_notify(clone!(
                #[weak]
                settings,
                move |p| {
                    let _ =
                        settings.set_uint("player-adaptive-start-bitrate", p.adaptive_bandwidth());
                }
            ));

            // Kinora: the player lives in the page's scrolling column, where
            // it gets no height of its own; size it 16:9 to the page width (at most
            // 70% of the page height). Fullscreen lifts this and lets it fill.
            self.clapper.add_tick_callback(clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move |clapper, _| {
                    let want = if obj.fullscreened() {
                        -1
                    } else {
                        let by_width = obj.width() as f64 * 9.0 / 16.0;
                        let cap = obj.height() as f64 * 0.7;
                        by_width.min(cap).max(120.0).round() as i32
                    };
                    if clapper.height_request() != want {
                        clapper.set_height_request(want);
                    }
                    glib::ControlFlow::Continue
                }
            ));

            // Kinora: "Up next" list and autoplay.
            self.up_next_list.bind_model(
                Some(&self.up_next_store),
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_panic]
                    move |item| {
                        let video = item
                            .downcast_ref::<Video>()
                            .expect("Up next items to be videos");
                        let video_item = VideoItem::new();
                        video_item.set_client(obj.client());
                        video_item.set_compact(true);
                        video_item.set_video(video);
                        // The item's own menu ("play externally", ...).
                        video_item.connect_local(
                            "play-video",
                            false,
                            clone!(
                                #[weak]
                                obj,
                                #[upgrade_or_default]
                                move |args| {
                                    let video: Video = args[1].get().expect("Video argument");
                                    let play_type: PlayType =
                                        args[2].get().expect("PlayType argument");
                                    obj.emit_by_name::<()>("play-video", &[&video, &play_type]);
                                    None
                                }
                            ),
                        );
                        gtk::ListBoxRow::builder()
                            .child(&video_item)
                            .build()
                            .upcast()
                    }
                ),
            );
            settings
                .bind("autoplay-next", &*self.autoplay_switch, "active")
                .build();
            player.connect_state_notify(clone!(
                #[weak]
                obj,
                move |player| {
                    obj.imp().maybe_autoplay(player);
                }
            ));

            // Set up player settings
            settings.bind("player-volume", &player, "volume").build();
            settings.bind("player-speed", &player, "speed").build();
            settings
                .bind("player-subtitles-enabled", &player, "subtitles-enabled")
                .build();

            // Display billboard upon volume or speed change
            player.connect_volume_notify(clone!(
                #[weak]
                billboard,
                move |_| {
                    billboard.announce_volume();
                }
            ));

            player.connect_speed_notify(clone!(
                #[weak]
                billboard,
                move |_| {
                    billboard.announce_speed();
                }
            ));

            player.connect_state_notify(clone!(
                #[weak]
                obj,
                move |_player| {
                    obj.notify_state();
                }
            ));

            player.connect_position_notify(clone!(
                #[weak]
                obj,
                move |_player| {
                    obj.notify_position();
                }
            ));

            let watch = player
                .property_expression("queue")
                .chain_property::<clapper::Queue>("current-item")
                .chain_property::<clapper::MediaItem>("duration")
                .watch(
                    Some(&player),
                    clone!(
                        #[weak]
                        obj,
                        move || {
                            obj.notify_duration();
                        }
                    ),
                );

            self.watch.replace(Some(watch));

            // Set initial bit rate from last observed bit rate.
            // Very crude approximation, but is simple to implement.
            // A more involved implementation could look at the history of bitrates.
            settings
                .bind(
                    "player-adaptive-start-bitrate",
                    &player,
                    "adaptive-start-bitrate",
                )
                .build();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VideoPage {
        const NAME: &'static str = "TFVideoPage";
        type Type = super::VideoPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            clapper_gtk::Video::ensure_type();
            clapper_gtk::Billboard::ensure_type();
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
            Utility::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for VideoPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup();
            self.setup_actions();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: std::sync::LazyLock<Vec<Signal>> = std::sync::LazyLock::new(|| {
                vec![
                    Signal::builder("display-channel")
                        .param_types([Channel::static_type()])
                        .build(),
                    Signal::builder("download-video")
                        .param_types([Video::static_type()])
                        .build(),
                    Signal::builder("remove-downloaded")
                        .param_types([Video::static_type()])
                        .build(),
                    Signal::builder("copy-video-url-to-clipboard")
                        .param_types([Video::static_type()])
                        .build(),
                    // Kinora: play a video from "Up next" (or autoplay).
                    Signal::builder("play-video")
                        .param_types([Video::static_type(), PlayType::static_type()])
                        .build(),
                ]
            });
            SIGNALS.as_ref()
        }

        fn dispose(&self) {
            if let Some(watch) = self.watch.borrow().as_ref() {
                watch.unwatch();
            }
        }
    }

    impl WidgetImpl for VideoPage {
        fn grab_focus(&self) -> bool {
            self.clapper.grab_focus()
        }
    }
    impl BinImpl for VideoPage {}
}
