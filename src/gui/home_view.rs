//! Kinora: Home, a featured banner over rows of videos.
//!
//! Home is composed here from what the backend already has: the account's
//! home feed (banner and "For you"), the watch history, Watch Later, the
//! subscriptions feed, and one row per topic chip of the account's Home.
//! Every row is a [`VideoRow`]; rows from extensions will be added the same
//! way (see `docs/design.md`).

gtk::glib::wrapper! {
    pub struct HomeView(ObjectSubclass<imp::HomeView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl HomeView {
    /// What the "Mixes" and "Your playlists" shelves show.
    pub fn set_playlists(
        &self,
        mixes: Vec<crate::gui::playlist_shelf::PlaylistInfo>,
        playlists: Vec<crate::gui::playlist_shelf::PlaylistInfo>,
    ) {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        let shelves = self.imp().shelves.borrow();
        for (shelf, items) in shelves.iter().zip([mixes, playlists]) {
            shelf.set_items(items);
        }
    }

    /// Scroll Home vertically (test hook).
    #[cfg(feature = "dev-hooks")]
    pub fn scroll_to(&self, y: f64) {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        self.imp().scroll_to(y);
    }
}

pub mod imp {
    use adw::prelude::*;
    use adw::subclass::prelude::*;
    use gdk::gio;
    use glib::Properties;
    use glib::clone;
    use glib::closure_local;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::CompositeTemplate;
    use gtk::glib;
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::sync::LazyLock;

    use crate::backend::Client;
    use crate::backend::FiniteVideoStore;
    use crate::backend::PlayType;
    use crate::backend::Video;
    use crate::gspawn;
    use crate::gui::featured_banner::FeaturedBanner;
    use crate::gui::video_row::VideoRow;

    /// How many videos a row built from a long list shows.
    const ROW_LENGTH: u32 = 30;
    /// How many topic rows Home shows at most.
    const MAX_TOPICS: u32 = 8;

    /// A topic row and what is needed to fill it once it scrolls into view.
    struct TopicRow {
        row: VideoRow,
        store: gio::ListStore,
        chip: u32,
        requested: Cell<bool>,
    }

    #[derive(CompositeTemplate, Properties, Default)]
    #[template(resource = "/ui/home_view.ui")]
    #[properties(wrapper_type = super::HomeView)]
    pub struct HomeView {
        #[template_child]
        scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        content: TemplateChild<gtk::Box>,
        #[template_child]
        banner: TemplateChild<FeaturedBanner>,
        #[template_child]
        rows: TemplateChild<gtk::Box>,
        #[template_child]
        topic_rows: TemplateChild<gtk::Box>,

        #[property(get, set = Self::set_client)]
        client: RefCell<Client>,
        /// Narrow window: smaller cards and gutters.
        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,
        /// The subscriptions feed, for the "From your subscriptions" row.
        #[property(get, set = Self::set_feed_model, nullable)]
        feed_model: RefCell<Option<gio::ListModel>>,

        /// The DJ can be started (its extension is on): show its strip.
        #[property(get, set)]
        dj_available: Cell<bool>,

        /// Mixes and playlists (see `playlist_shelf.rs`); empty shelves hide.
        pub(super) shelves: RefCell<Vec<crate::gui::playlist_shelf::PlaylistShelf>>,

        built: Cell<bool>,
        row_subscriptions: RefCell<Option<VideoRow>>,
        topics: RefCell<Vec<TopicRow>>,
        /// Counts rebuilds of the topic rows, so late answers can be dropped.
        topics_generation: Cell<u32>,
    }

    #[gtk::template_callbacks]
    impl HomeView {
        fn set_client(&self, client: Client) {
            self.client.replace(client);
            // The client is set once, when the window is built.
            if !self.built.replace(true) {
                self.build_rows();
            }
        }

        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);
            if compact {
                self.obj().add_css_class("compact");
            } else {
                self.obj().remove_css_class("compact");
            }
            let width = self.card_width();
            for shelf in self.shelves.borrow().iter() {
                shelf.set_card_width(width);
            }
            for parent in [&*self.rows, &*self.topic_rows] {
                let mut child = parent.first_child();
                while let Some(widget) = child {
                    if let Some(row) = widget.downcast_ref::<VideoRow>() {
                        row.set_card_width(width);
                    }
                    child = widget.next_sibling();
                }
            }
        }

        fn set_feed_model(&self, model: Option<gio::ListModel>) {
            if let Some(row) = self.row_subscriptions.borrow().as_ref() {
                row.set_model(model.as_ref().map(|m| {
                    gtk::SliceListModel::new(Some(m.clone()), 0, ROW_LENGTH)
                        .upcast::<gio::ListModel>()
                }));
            }
            self.feed_model.replace(model);
        }

        fn card_width(&self) -> i32 {
            if self.compact.get() { 232 } else { 272 }
        }

        #[template_callback]
        fn forward_play(&self, video: Video, play_type: PlayType) {
            self.obj()
                .emit_by_name::<()>("play-video", &[&video, &play_type]);
        }

        /// A row whose cards behave like the ones everywhere else in the app.
        fn new_row(&self, title: &str, model: Option<gio::ListModel>) -> VideoRow {
            let obj = self.obj();
            let row = VideoRow::new(title, &obj.client());
            row.set_card_width(self.card_width());
            row.set_model(model);
            row.connect_closure(
                "play-video",
                false,
                closure_local!(
                    #[weak]
                    obj,
                    move |_: VideoRow, video: Video, play_type: PlayType| {
                        obj.emit_by_name::<()>("play-video", &[&video, &play_type]);
                    }
                ),
            );
            for signal in ["copy-video-url", "download", "remove-downloaded"] {
                row.connect_closure(
                    signal,
                    false,
                    closure_local!(
                        #[weak]
                        obj,
                        move |_: VideoRow, video: Video| {
                            obj.emit_by_name::<()>(signal, &[&video]);
                        }
                    ),
                );
            }
            row
        }

        /// Show the start of one of the client's stores in a row, following
        /// the client when it swaps the store (it does while starting up).
        fn bind_store_row(&self, row: &VideoRow, property: &'static str) {
            let apply = clone!(
                #[weak]
                row,
                move |client: &Client| {
                    let store = client.property::<FiniteVideoStore>(property);
                    row.set_model(Some(
                        gtk::SliceListModel::new(Some(store), 0, ROW_LENGTH)
                            .upcast::<gio::ListModel>(),
                    ));
                }
            );
            let client = self.obj().client();
            apply(&client);
            client.connect_notify_local(Some(property), move |client, _| apply(client));
        }

        fn build_rows(&self) {
            let client = self.obj().client();
            let home = client.home_store();

            // The first recommendation is featured; the rest are "For you".
            home.connect_items_changed(clone!(
                #[weak(rename_to = s)]
                self,
                move |home, _, _, _| s.update_featured(home.upcast_ref())
            ));
            self.update_featured(home.upcast_ref());

            let for_you = self.new_row(
                &gettextrs::gettext("For you"),
                Some(gtk::SliceListModel::new(Some(home.clone()), 1, 100_000).upcast()),
            );
            for_you.connect_closure(
                "scroll-end",
                false,
                closure_local!(
                    #[weak]
                    client,
                    move |_: VideoRow| {
                        gspawn!(async move { client.load_more_home().await });
                    }
                ),
            );
            self.rows.append(&for_you);

            // Mixes and playlists: opening one starts the DJ with it.
            let shelves: Vec<_> = [
                gettextrs::gettext("Mixes"),
                gettextrs::gettext("Your playlists"),
            ]
            .iter()
            .map(|title| {
                let shelf = crate::gui::playlist_shelf::PlaylistShelf::new(
                    title,
                    &client,
                    self.card_width(),
                );
                let obj = self.obj().downgrade();
                shelf.connect_open(move |playlist| {
                    if let Some(obj) = obj.upgrade() {
                        let target = (playlist.id.as_str(), playlist.title.as_str()).to_variant();
                        let _ = obj.activate_action("win.dj-playlist", Some(&target));
                    }
                });
                self.rows.append(shelf.widget());
                shelf
            })
            .collect();
            self.shelves.replace(shelves);

            // The backend keeps no playback positions, so this cannot be a
            // true "Continue watching" yet: it is the watch history.
            let watch_again = self.new_row(&gettextrs::gettext("Watch again"), None);
            self.bind_store_row(&watch_again, "watched-store");
            self.rows.append(&watch_again);

            let watch_later = self.new_row(&gettextrs::gettext("Watch Later"), None);
            self.bind_store_row(&watch_later, "watch-later-store");
            self.rows.append(&watch_later);

            let subscriptions = self.new_row(&gettextrs::gettext("From your subscriptions"), None);
            self.rows.append(&subscriptions);
            self.row_subscriptions.replace(Some(subscriptions));
            let feed = self.feed_model.borrow().clone();
            self.set_feed_model(feed);

            // Topic rows follow the account's Home chips.
            client.home_chips().connect_items_changed(clone!(
                #[weak(rename_to = s)]
                self,
                move |_, _, _, _| s.rebuild_topic_rows()
            ));
            self.rebuild_topic_rows();

            let adj = self.scroller.vadjustment();
            adj.connect_value_changed(clone!(
                #[weak(rename_to = s)]
                self,
                move |_| s.load_visible_topics()
            ));
            adj.connect_changed(clone!(
                #[weak(rename_to = s)]
                self,
                move |_| s.load_visible_topics()
            ));
        }

        /// Feature the first recommendation. While Home reloads (its list is
        /// empty for a moment) the banner keeps what it shows.
        fn update_featured(&self, home: &gio::ListModel) {
            if let Some(video) = home.item(0).and_downcast::<Video>() {
                self.banner.set_video(Some(video));
            }
        }

        /// Fill the Mixes and Your playlists shelves (after Home's chips
        /// changed, i.e. after Home loaded).
        fn load_playlists(&self) {
            let generation = self.topics_generation.get();
            let client = self.obj().client();
            gspawn!(clone!(
                #[weak(rename_to = s)]
                self,
                async move {
                    let (mixes, playlists) = client.home_playlists().await;
                    if s.topics_generation.get() != generation {
                        return;
                    }
                    let info = |cards: Vec<kinora_core::PlaylistCard>| {
                        cards.into_iter().map(super::playlist_info).collect::<Vec<_>>()
                    };
                    s.obj().set_playlists(info(mixes), info(playlists));
                }
            ));
        }

        fn rebuild_topic_rows(&self) {
            self.topics_generation
                .set(self.topics_generation.get().wrapping_add(1));
            while let Some(child) = self.topic_rows.first_child() {
                self.topic_rows.remove(&child);
            }
            let chips = self.obj().client().home_chips();
            let mut topics = Vec::new();
            // The first chip is "All": that is the "For you" row.
            for chip in 1..chips.n_items().min(MAX_TOPICS + 1) {
                let Some(title) = chips
                    .item(chip)
                    .and_downcast::<gtk::StringObject>()
                    .map(|t| t.string())
                else {
                    continue;
                };
                // Mixes and Playlists hold no videos: they fill the shelves.
                if kinora_core::extension::builtin::home_chips::PLAYLIST_ONLY_CHIPS
                    .contains(&title.as_str())
                {
                    continue;
                }
                let store = gio::ListStore::new::<Video>();
                let row = self.new_row(&title, Some(store.clone().upcast()));
                row.set_loading(true);
                self.topic_rows.append(&row);
                topics.push(TopicRow {
                    row,
                    store,
                    chip,
                    requested: Cell::new(false),
                });
            }
            self.topics.replace(topics);
            self.load_playlists();
            // Rows are placed on the next layout; look then.
            glib::idle_add_local_once(clone!(
                #[weak(rename_to = s)]
                self,
                move || s.load_visible_topics()
            ));
        }

        /// Fetch the topic rows that are on screen or about to be: a topic
        /// nobody scrolls to costs nothing.
        fn load_visible_topics(&self) {
            if !self.scroller.is_mapped() {
                return;
            }
            let adj = self.scroller.vadjustment();
            let limit = adj.value() + adj.page_size() * 1.5;
            let generation = self.topics_generation.get();
            for topic in self.topics.borrow().iter() {
                if topic.requested.get() {
                    continue;
                }
                let Some(bounds) = topic.row.compute_bounds(&*self.content) else {
                    continue;
                };
                if bounds.y() as f64 > limit {
                    break;
                }
                topic.requested.set(true);
                let client = self.obj().client();
                let (row, store, chip) = (topic.row.clone(), topic.store.clone(), topic.chip);
                gspawn!(clone!(
                    #[weak(rename_to = s)]
                    self,
                    async move {
                        let videos = client.home_chip_videos(chip).await;
                        if s.topics_generation.get() != generation {
                            return;
                        }
                        store.splice(0, store.n_items(), &videos);
                        row.set_loading(false);
                    }
                ));
            }
        }

        #[cfg(feature = "dev-hooks")]
        pub(super) fn scroll_to(&self, y: f64) {
            self.scroller.vadjustment().set_value(y);
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HomeView {
        const NAME: &'static str = "TFHomeView";
        type Type = super::HomeView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            FeaturedBanner::ensure_type();
            VideoRow::ensure_type();
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for HomeView {
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

    impl WidgetImpl for HomeView {
        fn map(&self) {
            self.parent_map();
            self.load_visible_topics();
        }
    }
    impl BinImpl for HomeView {}
}

/// A core playlist card as the shelf shows it.
fn playlist_info(card: kinora_core::PlaylistCard) -> crate::gui::playlist_shelf::PlaylistInfo {
    use crate::gui::playlist_shelf::{PlaylistInfo, PlaylistKind};
    PlaylistInfo {
        id: card.id,
        title: card.title,
        owner: card.owner,
        count: card.video_count,
        kind: match card.kind {
            kinora_core::PlaylistKind::Mix => PlaylistKind::Mix,
            kinora_core::PlaylistKind::Playlist => PlaylistKind::Playlist,
            kinora_core::PlaylistKind::Album => PlaylistKind::Album,
        },
        thumbnail: card.thumbnail,
    }
}
