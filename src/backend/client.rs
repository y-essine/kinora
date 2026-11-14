use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::StreamExt;
use gdk::gdk_pixbuf::Pixbuf;
use gdk::gio::prelude::{FileExt, ListModelExt, ListModelExtManual};
use gdk::gio::{File, MemoryInputStream, SettingsBindFlags};
use gdk::prelude::{SettingsExt, SettingsExtManual};
use glib::object::ObjectExt;
use glib::{Bytes, clone};
use glib::{Object, subclass::types::ObjectSubclassIsExt};
use gtk::prelude::FilterExt;
use pcore::{ChannelId, Provider, ProviderExt, VideoId};
use tokio::sync::MutexGuard;
use url::Url;

use crate::backend::{FilterStore, FiniteVideoStore, SubscriptionsStore};
use crate::store::{Cache, Store, export, import};
use crate::{gspawn, tspawn};

use super::extension_values::extension_values;
use super::{Channel, Filter, SearchItem, SearchStream, Video, VideoFetcherStream};

gtk::glib::wrapper! {
    pub struct Client(ObjectSubclass<imp::Client>);
}

const PIPED_INSTANCES_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

impl std::default::Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Client {
        Object::builder().build()
    }

    pub fn setup(&self) -> Result<(), Box<dyn std::error::Error>> {
        let client = reqwest::Client::builder()
            .user_agent(format!(
                "{}/{}",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .expect("Failed to build reqwest client");

        self.imp().reqwest_client.borrow_mut().replace(client);

        let mut sqlite_path = glib::user_data_dir();
        sqlite_path.push(crate::migrate::DIR);
        sqlite_path.push("data.sqlite");
        let store = Store::new(sqlite_path)?;
        self.imp().store.borrow_mut().replace(store);
        self.load_data()?;

        let mut cache_path = glib::user_cache_dir();
        cache_path.push(crate::migrate::DIR);
        // Kinora: the core keeps its files next to the thumbnail cache.
        let core = kinora_core::Core::new(kinora_core::Paths::new(cache_path.clone()));
        self.imp().core.borrow_mut().replace(Arc::new(core));
        let cache = Cache::new(cache_path);
        self.imp().cache.borrow_mut().replace(cache);

        let settings = &self.imp().settings;
        settings.connect_changed(
            Some("piped-api-urls"),
            clone!(
                #[weak(rename_to = s)]
                self,
                move |_, _| {
                    gspawn!(async move { s.update_internal().await });
                }
            ),
        );
        settings.connect_changed(
            Some("peertube-search-api"),
            clone!(
                #[weak(rename_to = s)]
                self,
                move |_, _| {
                    gspawn!(async move { s.update_internal().await });
                }
            ),
        );
        settings.connect_changed(
            Some("youtube-mode"),
            clone!(
                #[weak(rename_to = s)]
                self,
                move |_, _| {
                    gspawn!(async move { s.update_internal().await });
                }
            ),
        );
        settings.connect_changed(
            Some("enable-source-youtube"),
            clone!(
                #[weak(rename_to = s)]
                self,
                move |_, _| {
                    gspawn!(async move { s.update_internal().await });
                }
            ),
        );
        settings.connect_changed(
            Some("enable-source-peertube"),
            clone!(
                #[weak(rename_to = s)]
                self,
                move |_, _| {
                    gspawn!(async move { s.update_internal().await });
                }
            ),
        );
        // Kinora: the extensions that are switched off, and their settings.
        self.load_extension_settings();
        for key in ["disabled-extensions", "extension-settings"] {
            settings.connect_changed(
                Some(key),
                clone!(
                    #[weak(rename_to = s)]
                    self,
                    move |_, _| {
                        s.load_extension_settings();
                        // The account or the chips may have been switched on or off.
                        s.account_changed();
                    }
                ),
            );
        }
        // Kinora: follow the YouTube account being connected or disconnected.
        self.set_account_label(account_label(settings));
        self.set_has_account(self.core().account_active(&self.account_config()));
        settings.connect_changed(
            Some("account-cookies-command"),
            clone!(
                #[weak(rename_to = s)]
                self,
                move |_, _| s.account_changed()
            ),
        );
        settings.connect_changed(
            Some("account-label"),
            clone!(
                #[weak(rename_to = s)]
                self,
                move |settings, _| s.set_account_label(account_label(settings))
            ),
        );

        settings
            .bind("only-videos-yesterday", &self.filter_store(), "hide-today")
            .flags(SettingsBindFlags::DEFAULT)
            .build();
        settings
            .bind("remove-short-videos", &self.filter_store(), "hide-shorts")
            .flags(SettingsBindFlags::DEFAULT)
            .build();
        settings
            .bind(
                "remove-requires-payment",
                &self.filter_store(),
                "hide-requires-payment",
            )
            .flags(SettingsBindFlags::DEFAULT)
            .build();
        settings
            .bind("remove-watched", &self.filter_store(), "hide-watched")
            .flags(SettingsBindFlags::DEFAULT)
            .build();
        settings
            .bind("filter-enabled", &self.filter_store(), "enabled")
            .flags(SettingsBindFlags::DEFAULT)
            .build();

        Ok(())
    }

    pub async fn setup_async(&self) {
        let settings = &self.imp().settings;
        if settings.boolean("piped-automatically-manage-instances")
            && settings.string("youtube-mode") != "rustypipe"
        {
            self.refresh_piped_instances().await;
        }
        self.update_internal().await;
    }

    pub fn cleanup(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut binding = self.imp().store.borrow_mut();
        let store = binding.as_mut().expect("Store to be set up");

        store.trim_unused()?;

        let mut binding = self.imp().cache.borrow_mut();
        let cache = binding.as_mut().expect("Store to be set up");

        cache.purge_old();

        self.clear_title_cache();

        Ok(())
    }

    pub fn backup(&self, file: File) -> Result<(), Box<dyn std::error::Error>> {
        self.imp()
            .store
            .borrow()
            .as_ref()
            .expect("Store to be set up")
            .backup(file.path().expect("File to have a path"))?;
        Ok(())
    }

    pub fn restore(&self, file: File) -> Result<(), Box<dyn std::error::Error>> {
        self.imp()
            .store
            .borrow_mut()
            .as_mut()
            .expect("Store to be set up")
            .restore(file.path().expect("File to have a path"))?;
        self.set_subscriptions_store(SubscriptionsStore::default());
        self.set_watch_later_store(FiniteVideoStore::default());
        self.set_downloaded_store(FiniteVideoStore::default());
        self.set_watched_store(FiniteVideoStore::with_sort(
            crate::backend::finite_video_store::VideoSort::WatchedDescending,
        ));
        self.set_filter_store(FilterStore::default());
        self.load_data()?;
        Ok(())
    }

    fn load_data(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut binding = self.imp().store.borrow_mut();
        let store = binding.as_mut().expect("Store to be set up");

        let subscriptions = store.subscriptions()?;
        let watch_later = store.watch_later_list()?;
        let downloaded = store.downloaded_list()?;
        let filters = store.filters()?;
        let watched = store.watch_history()?;

        drop(binding);

        // All the data is already loaded from sqlite.
        // Therefore, the `_internal` functions can be used without interacting with the sqlite store.
        // This also speeds up startup a lot.
        for (vid, timestamp) in watched {
            self.watched_internal(self.construct_video(vid), timestamp, false);
        }
        for vid in watch_later {
            self.toggle_watch_later_internal(self.construct_video(vid), false);
        }
        for (vid, path) in downloaded {
            self.downloaded_internal(self.construct_video(vid), path, false);
        }
        for sub in subscriptions {
            self.toggle_subscription_internal(self.construct_channel(sub), false);
        }
        for fil in filters {
            self.toggle_filter_internal(fil, false);
        }

        Ok(())
    }

    async fn refresh_piped_instances(&self) {
        let client = self
            .imp()
            .reqwest_client
            .borrow()
            .clone()
            .expect("Client to be set up");
        let settings = &self.imp().settings;

        if let Ok(Ok(instances)) = tspawn!(async move {
            tokio::time::timeout(
                PIPED_INSTANCES_TIMEOUT,
                ppiped::PipedProvider::find_instances(&client),
            )
            .await
        })
        .await
        .expect("Failed to spawn tokio")
        {
            if instances.is_empty() {
                log::warn!("Got an empty list of Piped instances. Keeping the old instance list");
                return;
            }

            let instances_string = instances.iter().map(|u| u.as_str()).collect::<Vec<_>>();
            log::info!("Updating the Piped instance list to: {instances_string:?}.",);
            let _ = settings.set("piped-api-urls", instances_string);
        } else {
            log::warn!(
                "Failed to fetch Piped the instance list. Using the previously saved instance list."
            );
        }
    }

    async fn update_internal(&self) {
        log::trace!("Updating the internal providers");
        self.imp()
            .setup_finished_watch_channel
            .send_modify(|b: &mut bool| *b = false);

        let settings = &self.imp().settings;
        let client = self
            .imp()
            .reqwest_client
            .borrow()
            .clone()
            .expect("Client to be set up");

        let source_youtube = settings.boolean("enable-source-youtube");
        let source_peertube = settings.boolean("enable-source-peertube");

        let mut internal = self.internal().await;
        *internal = Default::default();

        if source_youtube {
            // TODO: Some kind of error when anything is not a valid URL?
            let piped_apis = settings
                .get::<Vec<String>>("piped-api-urls")
                .into_iter()
                .flat_map(|u| Url::parse(&u).ok())
                .collect();
            let piped = ppiped::PipedProvider::new(
                client.clone(),
                piped_apis,
                ppiped::RequestOrder::Random,
            );

            let mut rustypipe_path = glib::user_data_dir();
            rustypipe_path.push(crate::migrate::DIR);
            rustypipe_path.push("rustypipe");
            // TODO: Error
            let _ = std::fs::create_dir_all(&rustypipe_path);
            let rustypipe = prustypipe::RustyPipeProvider::new(rustypipe_path)
                .expect("Failed to setup rustypipe");

            let direct = rustypipe
                .fallback_to(pyoutuberss::YouTubeRssProvider::new(client.clone()))
                .fallback_to(pytdlp::YtdlpYouTubeProvider::new());

            let youtube_mode = settings.string("youtube-mode");
            match &youtube_mode[..] {
                "piped" => {
                    internal.register(piped);
                }
                "rustypipe" => {
                    internal.register(direct);
                }
                "piped-then-rustypipe" => {
                    internal.register(piped.fallback_to(direct));
                }
                "rustypipe-then-piped" => {
                    internal.register(direct.fallback_to(piped));
                }
                // Legacy values
                "always-directly" => {
                    internal.register(direct.fallback_to(piped));
                    let _ = settings.set_string("youtube-mode", "rustypipe-then-piped");
                }
                "always-piped" => {
                    internal.register(piped);
                    let _ = settings.set_string("youtube-mode", "piped");
                }
                v => {
                    unreachable!("Unknown youtube-mode: {v}");
                }
            }
        }

        if source_peertube {
            let peertube_search_api: String = settings.get("peertube-search-api");
            let peertube = ppeertube::PeertubeProvider::new(
                client,
                Url::parse(&peertube_search_api).unwrap_or_else(|_| {
                    Url::parse("https://sepiasearch.org/").expect("Sepiasearch to be valid URL")
                }),
            );
            internal.register(peertube);
        }

        self.imp()
            .setup_finished_watch_channel
            .send_modify(|b: &mut bool| *b = true);
    }

    async fn internal(&self) -> MutexGuard<'_, papi::PipelineApi> {
        // TODO: May panic if multiple reloads/searches in parallel.
        self.imp().internal.lock().await
    }

    async fn bytes_for_url(&self, url: Url) -> Result<Bytes, reqwest::Error> {
        let client = self
            .imp()
            .reqwest_client
            .borrow()
            .clone()
            .expect("Client to be set up");
        log::trace!("Fetching data for url {url}");
        tspawn!(async move { client.get(url).send().await?.bytes().await })
            .await
            .expect("Failed to spawn tokio")
            .map(Bytes::from_owned)
    }

    async fn pixbuf_for_url(&self, url: Url) -> Option<Pixbuf> {
        let bytes = self.bytes_for_url(url).await.ok()?;
        let stream = MemoryInputStream::from_bytes(&bytes);
        Pixbuf::from_stream_future(&stream).await.ok()
    }

    /// Kinora: a picture by its address, through the thumbnail cache (for
    /// things that are not videos: playlist covers).
    pub async fn picture(&self, url: Url) -> Option<Pixbuf> {
        let cache = self.imp().cache.borrow().clone();
        cache
            .as_ref()
            .expect("Cache to be set up")
            .get_or_fetch(url, |u| async { self.pixbuf_for_url(u).await })
            .await
    }

    async fn populate_avatar(&self, channel: &Channel) {
        if let Some(url) = channel.avatar_url() {
            let cache = self.imp().cache.borrow().clone();
            if let Some(pixbuf) = cache
                .as_ref()
                .expect("Cache to be set up")
                .get_or_fetch(url, |u| async { self.pixbuf_for_url(u).await })
                .await
            {
                channel.set_avatar(pixbuf);
            }
        }
    }

    pub async fn populate_thumbnail(&self, video: &Video) {
        if let Some(url) = video.thumbnail_url()
            && video.thumbnail().is_none()
        {
            let cache = self.imp().cache.borrow().clone();
            if let Some(pixbuf) = cache
                .as_ref()
                .expect("Cache to be set up")
                .get_or_fetch(url, |u| async { self.pixbuf_for_url(u).await })
                .await
            {
                video.set_thumbnail(Some(pixbuf));
            }
        }

        // If the video was cached, it could be that the thumbnail URL is not valid anymore after loading from the store.
        // Retry after populating video information.
        if video.thumbnail().is_none() {
            if let Err(e) = self.populate_video_information(&video.id()).await {
                log::warn!(
                    "Failed to populate video information for video '{}': {}",
                    video.title(),
                    e
                );
            } else {
                if let Some(url) = video.thumbnail_url()
                    && video.thumbnail().is_none()
                {
                    let cache = self.imp().cache.borrow().clone();
                    if let Some(pixbuf) = cache
                        .as_ref()
                        .expect("Cache to be set up")
                        .get_or_fetch(url, |u| async { self.pixbuf_for_url(u).await })
                        .await
                    {
                        video.set_thumbnail(Some(pixbuf));
                    }
                }
            }
        }
    }

    pub(super) fn construct_channel(&self, channel: pcore::Channel) -> Channel {
        let mut cache = self.imp().channel_cache.borrow_mut();
        if let Some(cached) = cache.get(&channel.id).and_then(|r| r.upgrade()) {
            if cached.channel() != channel {
                let requires_avatar_update = cached.avatar_url() != channel.avatar;

                cached.update(channel);

                let mut binding = self.imp().store.borrow_mut();
                let store = binding.as_mut().expect("Store to be set up");
                // TODO: Error handling
                let _ = store.update_channel_if_exists(&cached.channel());

                if requires_avatar_update {
                    gspawn!(
                        clone!(
                            #[weak(rename_to = s)]
                            self,
                            #[weak]
                            cached,
                            async move {
                                s.populate_avatar(&cached).await;
                            }
                        ),
                        glib::Priority::LOW
                    );
                }
            }

            cached
        } else {
            let id = channel.id.clone();
            let object = Channel::new(channel);
            cache.insert(id.clone(), object.downgrade());

            gspawn!(
                clone!(
                    #[weak(rename_to = s)]
                    self,
                    #[weak]
                    object,
                    async move {
                        s.populate_avatar(&object).await;
                        // If the channel was cached, it could be that the avatar URL is not valid anymore after loading from the store.
                        // Retry setting that information later.
                        if object.avatar().is_none()
                            && let Err(e) = s.populate_channel_information(&id).await
                        {
                            log::warn!(
                                "Failed to populate channel information for channel '{}': {}",
                                object.name(),
                                e
                            );
                        }
                    }
                ),
                glib::Priority::LOW
            );

            object
        }
    }

    pub(super) fn construct_video(&self, mut video: pcore::Video) -> Video {
        let mut cache = self.imp().video_cache.borrow_mut();
        if let Some(cached) = cache.get(&video.id).and_then(|r| r.upgrade()) {
            if cached.video() != video {
                let requires_thumbnail_update = cached.thumbnail_url() != video.thumbnail;
                let new_uploader = video.uploader.clone();

                // Preserve the originally cached title when video metadata is refreshed.
                // External sources may provide incomplete or placeholder titles during metadata
                // updates. Keeping the cached title ensures UI consistency and guards against
                // regressions from external metadata changes.
                let title_cache = self.imp().title_cache.borrow();
                let cached_title = title_cache.get(&video.id).cloned();
                drop(title_cache);

                if let Some(title) = cached_title {
                    video.title = title;
                }

                cached.update(video);

                // Kinora: "Up next" videos can arrive without their channel.
                if !new_uploader.id.id.is_empty() && cached.uploader().id() != new_uploader.id {
                    cached.set_uploader(self.construct_channel(new_uploader));
                }

                let mut binding = self.imp().store.borrow_mut();
                let store = binding.as_mut().expect("Store to be set up");
                // TODO: Error handling
                let _ = store.update_video_if_exists(&cached.video());

                if requires_thumbnail_update {
                    cached.set_thumbnail(None::<&Pixbuf>);
                }
            }

            cached
        } else {
            let id = video.id.clone();
            let uploader = video.uploader.clone();
            let title = video.title.clone();
            let object = Video::new(video, &self.construct_channel(uploader));
            cache.insert(id.clone(), object.downgrade());

            // Prune title_cache entries whose video_cache weak reference can no longer
            // be upgraded, ensuring titles are dropped when the Video is reclaimed.
            cache.retain(|_, v| v.upgrade().is_some());
            let mut title_cache = self.imp().title_cache.borrow_mut();
            title_cache.retain(|k, _| cache.contains_key(k));
            title_cache.insert(id.clone(), title);

            object
        }
    }

    /// Clears the title cache. This is automatically called by [`Self::cleanup()`]
    /// to reclaim memory from stale title entries.
    pub fn clear_title_cache(&self) {
        self.imp().title_cache.borrow_mut().clear();
    }

    pub(super) fn construct_search_item(&self, item: pcore::SearchItem) -> SearchItem {
        match item {
            pcore::SearchItem::Video(v) => SearchItem::for_video(&self.construct_video(v)),
            pcore::SearchItem::Channel(c) => SearchItem::for_channel(&self.construct_channel(c)),
        }
    }

    pub async fn videos_for_channels(
        &self,
        channel_ids: &[ChannelId],
        filter: bool,
    ) -> VideoFetcherStream {
        self.await_setup_finished().await;
        let filter_store = if filter {
            self.filter_store()
        } else {
            FilterStore::default()
        };
        VideoFetcherStream::new(
            self.internal().await.videos_for_channels(channel_ids),
            self,
            &filter_store,
        )
    }

    /// Search all enabled platforms, or only `platform` (e.g. "youtube") if given.
    // Kinora: the platform filter is applied on the merged stream, so that the platform's
    // loading state and errors are hidden together with its results.
    pub async fn search(&self, query: &str, platform: Option<&str>) -> SearchStream {
        self.await_setup_finished().await;
        let stream = self.internal().await.search(query);
        let stream: Pin<Box<dyn pcore::SearchStream>> = match platform {
            Some(platform) => {
                let platform = platform.to_owned();
                Box::pin(
                    stream.filter(move |r| futures::future::ready(search_result_on(r, &platform))),
                )
            }
            None => stream,
        };
        SearchStream::new(stream, self)
    }

    pub async fn populate_video_information(&self, video_id: &VideoId) -> Result<(), pcore::Error> {
        self.await_setup_finished().await;
        let internal = self.imp().internal.clone();
        let video_id = video_id.clone();
        let core_video = tspawn!(async move {
            let internal = internal.lock().await;
            let info = internal.video_information(&video_id);
            drop(internal);
            info.await
        })
        .await
        .expect("Failed to spawn tokio")?;
        // Already in cache
        self.construct_video(core_video);
        Ok(())
    }

    pub async fn populate_channel_information(
        &self,
        channel_id: &ChannelId,
    ) -> Result<(), pcore::Error> {
        self.await_setup_finished().await;
        let internal = self.imp().internal.clone();
        let channel_id = channel_id.clone();
        let core_channel = tspawn!(async move {
            let internal = internal.lock().await;
            let info = internal.channel_information(&channel_id);
            drop(internal);
            info.await
        })
        .await
        .expect("Failed to spawn tokio")?;
        // Already in cache
        self.construct_channel(core_channel);
        Ok(())
    }

    // ---- Kinora: the signed-in YouTube account ----

    /// The core (see `crates/kinora-core`), shared with tokio tasks. The
    /// preferences list the extensions with `core().extensions().settings_panels()`
    /// and switch them through the `disabled-extensions` and
    /// `extension-settings` settings, which the Client passes on.
    pub fn core(&self) -> Arc<kinora_core::Core> {
        self.imp().core.borrow().clone().expect("Core to be set up")
    }

    /// Hand the extension settings from GSettings to the core.
    fn load_extension_settings(&self) {
        let settings = &self.imp().settings;
        let core = self.core();
        core.extensions()
            .set_disabled(settings.strv("disabled-extensions"));
        core.extensions()
            .set_values(extension_values(&settings.value("extension-settings")));
    }

    /// The account was connected or disconnected, or its extension (or the
    /// chips) switched on or off: update the account state and reload Home.
    fn account_changed(&self) {
        let connected = self.core().account_active(&self.account_config());
        let changed = connected != self.has_account();
        self.set_account_label(account_label(&self.imp().settings));
        self.set_has_account(connected);
        if changed || connected {
            self.reset_home();
            if connected {
                let s = self.clone();
                gspawn!(async move { s.refresh_home().await });
            }
        }
    }

    fn account_config(&self) -> kinora_core::AccountConfig {
        kinora_core::AccountConfig::new(self.imp().settings.string("account-cookies-command"))
    }

    fn playback_config(&self) -> kinora_core::PlaybackConfig {
        kinora_core::PlaybackConfig {
            fast_start: self.imp().settings.boolean("fast-start"),
        }
    }

    /// Forget everything Home shows, and drop loads that are still running.
    fn reset_home(&self) {
        let imp = self.imp();
        imp.home_gen.set(imp.home_gen.get().wrapping_add(1));
        imp.home_loading_more.set(false);
        imp.home_next.take();
        self.set_home_chips_list(&[]);
        self.home_store().clear();
        self.set_home_error(String::new());
        self.set_home_loading(false);
    }

    /// Show the filter bar's chips; the first one ("All") is selected.
    fn set_home_chips_list(&self, chips: &[kinora_core::HomeChip]) {
        let imp = self.imp();
        *imp.home_chip_tokens.borrow_mut() = chips.iter().map(|c| c.token.clone()).collect();
        self.set_home_chip_selected(0);
        let titles: Vec<gtk::StringObject> = chips
            .iter()
            .map(|c| gtk::StringObject::new(&c.title))
            .collect();
        let model = self.home_chips();
        model.splice(0, model.n_items(), &titles);
    }

    /// Reload the Home page from the account's YouTube home feed.
    pub async fn refresh_home(&self) {
        if self.home_loading() {
            return;
        }
        if !self.has_account() {
            self.reset_home();
            return;
        }
        // Reloading keeps the chosen filter.
        match self.home_chip(self.home_chip_selected()) {
            Some((index, Some(token))) => self.load_home(Some((index, token))).await,
            _ => self.load_home(None).await,
        }
    }

    /// Index and continuation token of a chip ("All" has none).
    fn home_chip(&self, index: u32) -> Option<(u32, Option<String>)> {
        let tokens = self.imp().home_chip_tokens.borrow();
        tokens.get(index as usize).map(|t| (index, t.clone()))
    }

    /// Show a chip's feed in Home ("All" is the first chip).
    pub async fn select_home_chip(&self, index: u32) {
        if index == self.home_chip_selected() || !self.has_account() {
            return;
        }
        match self.home_chip(index) {
            Some((index, Some(token))) => {
                self.set_home_chip_selected(index);
                self.load_home(Some((index, token))).await;
            }
            // "All" has no token: load the plain home page again.
            Some((_, None)) => {
                self.set_home_chip_selected(0);
                self.home_store().clear();
                self.load_home(None).await;
            }
            None => {}
        }
    }

    /// Kinora: the mixes and playlists Home shows on its shelves, for the DJ:
    /// those that came with the Home page, the "Mixes" and "Playlists" chip
    /// feeds, and the account's own playlists (first). Empty without an account.
    pub async fn home_playlists(
        &self,
    ) -> (Vec<kinora_core::PlaylistCard>, Vec<kinora_core::PlaylistCard>) {
        use kinora_core::extension::builtin::home_chips::PLAYLIST_ONLY_CHIPS;
        use kinora_core::PlaylistKind;
        if !self.has_account() {
            return Default::default();
        }
        let imp = self.imp();
        let chips = self.home_chips();
        let tokens: Vec<String> = {
            let tokens = imp.home_chip_tokens.borrow();
            (0..chips.n_items())
                .filter(|&i| {
                    chips
                        .item(i)
                        .and_then(|o| glib::object::Cast::downcast::<gtk::StringObject>(o).ok())
                        .is_some_and(|t| PLAYLIST_ONLY_CHIPS.contains(&t.string().as_str()))
                })
                .filter_map(|i| tokens.get(i as usize).cloned().flatten())
                .collect()
        };
        let from_home = imp.home_playlists.borrow().clone();
        let (core, account) = (self.core(), self.account_config());
        let started = std::time::Instant::now();
        let (from_chips, mine) = tspawn!(async move {
            let mut cards = Vec::new();
            for token in tokens {
                match core.home_with_playlists(&account, Some(&token)).await {
                    Ok(page) => cards.extend(page.playlists),
                    Err(e) => log::warn!("Home playlists: a chip feed failed: {e}"),
                }
            }
            let mine = core.account_playlists(&account).await.unwrap_or_else(|e| {
                log::warn!("Home playlists: the account's playlists failed: {e}");
                Vec::new()
            });
            (cards, mine)
        })
        .await
        .expect("Failed to spawn tokio");

        let mut seen = std::collections::HashSet::new();
        let (mut mixes, mut playlists) = (Vec::new(), Vec::new());
        for card in mine.into_iter().chain(from_home).chain(from_chips) {
            if !seen.insert(card.id.clone()) {
                continue;
            }
            match card.kind {
                PlaylistKind::Mix => mixes.push(card),
                _ => playlists.push(card),
            }
        }
        log::info!(
            "Home: {} mixes and {} playlists in {:.1} s",
            mixes.len(),
            playlists.len(),
            started.elapsed().as_secs_f64()
        );
        (mixes, playlists)
    }

    /// Kinora: the first videos of a chip's feed, without showing them in Home
    /// (Home's topic rows). Empty for "All", an unknown chip, or on an error.
    pub async fn home_chip_videos(&self, index: u32) -> Vec<Video> {
        let Some((_, Some(token))) = self.home_chip(index) else {
            return Vec::new();
        };
        // A chip's feed, without replacing the Home list (see `load_home`).
        let (core, account) = (self.core(), self.account_config());
        let result = tspawn!(async move { core.home(&account, Some(&token)).await })
            .await
            .expect("Failed to spawn tokio");
        match result {
            Ok(page) => page
                .videos
                .into_iter()
                .map(|v| self.construct_video(v))
                .collect(),
            Err(e) => {
                log::warn!("Failed to load a Home topic: {e}");
                Vec::new()
            }
        }
    }

    /// Load the first page of Home: the plain home page, or a chip's feed.
    /// A load started while another is running replaces it.
    async fn load_home(&self, chip: Option<(u32, String)>) {
        let imp = self.imp();
        let generation = imp.home_gen.get().wrapping_add(1);
        imp.home_gen.set(generation);
        imp.home_loading_more.set(false);
        self.set_home_loading(true);
        self.set_home_error(String::new());
        if chip.is_some() {
            // Show the spinner instead of the previous chip's videos.
            self.home_store().clear();
        }
        let started = std::time::Instant::now();

        let (core, account) = (self.core(), self.account_config());
        let token = chip.as_ref().map(|(_, token)| token.clone());
        let result = tspawn!(async move { core.home(&account, token.as_deref()).await })
            .await
            .expect("Failed to spawn tokio");

        // Superseded by another load (or the account changed) meanwhile.
        if imp.home_gen.get() != generation {
            return;
        }
        let page = match result {
            Ok(page) => page,
            Err(e) => {
                log::error!("Failed to load the home feed: {e}");
                self.set_home_error(e.to_string());
                self.set_home_loading(false);
                return;
            }
        };

        let videos: Vec<Video> = page
            .videos
            .into_iter()
            .map(|v| self.construct_video(v))
            .collect();
        let store = self.home_store();
        store.clear();
        for video in &videos {
            store.add(video.clone());
        }
        if chip.is_none() {
            *imp.home_playlists.borrow_mut() = page.playlists;
        }
        match &chip {
            Some((index, _)) => self.set_home_chip_selected(*index),
            None => self.set_home_chips_list(&page.chips),
        }
        *imp.home_next.borrow_mut() = page.next;
        log::info!(
            "Home: {} videos in {:.1} s",
            videos.len(),
            started.elapsed().as_secs_f64()
        );
        if videos.is_empty() {
            self.set_home_error(if chip.is_some() {
                "YouTube has no videos for this filter.".to_owned()
            } else {
                "YouTube returned no recommendations. Watch a few videos on the account first."
                    .to_owned()
            });
        }
        self.set_home_loading(false);

        // The listing has no upload dates.
        self.fill_in_details_later(videos);
    }

    /// Append the next page of Home (when its end is reached).
    pub async fn load_more_home(&self) {
        let imp = self.imp();
        if self.home_loading() || imp.home_loading_more.get() {
            return;
        }
        let Some(token) = imp.home_next.borrow().clone() else {
            return;
        };
        imp.home_loading_more.set(true);
        let generation = imp.home_gen.get();
        let started = std::time::Instant::now();

        let (core, account) = (self.core(), self.account_config());
        let result = tspawn!(async move { core.home_more(&account, &token).await })
            .await
            .expect("Failed to spawn tokio");

        if imp.home_gen.get() != generation {
            return;
        }
        imp.home_loading_more.set(false);
        let page = match result {
            Ok(page) => page,
            Err(e) => {
                log::warn!("Failed to load more of the home feed: {e}");
                return;
            }
        };

        let store = self.home_store();
        let before = store.n_items();
        let videos: Vec<Video> = page
            .videos
            .into_iter()
            .map(|v| self.construct_video(v))
            .collect();
        for video in &videos {
            store.add(video.clone());
        }
        *imp.home_next.borrow_mut() = page.next;
        log::info!(
            "Home: {} more videos in {:.1} s",
            store.n_items() - before,
            started.elapsed().as_secs_f64()
        );
        self.fill_in_details_later(videos);
    }

    /// Load the full information of videos that came from a short listing (upload
    /// date, views, channel), one after another in the background. That can swap
    /// the thumbnail URL, which clears the loaded thumbnail, so reload it too.
    fn fill_in_details_later(&self, videos: Vec<Video>) {
        gspawn!(clone!(
            #[weak(rename_to = s)]
            self,
            async move {
                use futures::StreamExt;
                // A few at a time, so "Up next" fills in within seconds.
                futures::stream::iter(videos)
                    .for_each_concurrent(4, |video| {
                        let s = s.clone();
                        async move {
                            if let Err(e) = s.populate_video_information(&video.id()).await {
                                log::debug!("No extra information for {}: {e}", video.id().id);
                            }
                            s.populate_thumbnail(&video).await;
                        }
                    })
                    .await;
            }
        ));
    }

    /// Subscribe to every channel the connected YouTube account is subscribed
    /// to. Channels already subscribed to are kept, and nothing is unsubscribed.
    /// Returns how many channels were added and how many the account has.
    pub async fn import_account_subscriptions(&self) -> Result<(usize, usize), String> {
        if !self.has_account() {
            return Err("No YouTube account is connected.".to_owned());
        }
        if self.importing_subscriptions() {
            return Err("Subscriptions are already being imported.".to_owned());
        }
        self.set_importing_subscriptions(true);

        // The account extension's menu item does the YouTube part.
        use kinora_core::extension::{ActionOutcome, MenuTarget, builtin::account};
        let (core, config) = (self.core(), self.account_config());
        let result = tspawn!(async move {
            core.activate(
                &config,
                account::ID,
                account::IMPORT_SUBSCRIPTIONS,
                MenuTarget::None,
            )
            .await
        })
        .await
        .expect("Failed to spawn tokio");
        self.set_importing_subscriptions(false);

        let channels = match result {
            Ok(ActionOutcome::Subscribe(channels)) => channels,
            Ok(_) => Vec::new(),
            Err(e) => {
                log::error!("Failed to load the account's subscriptions: {e}");
                return Err(e.to_string());
            }
        };
        let total = channels.len();
        let known: std::collections::HashSet<pcore::ChannelId> = self
            .subscriptions()
            .iter()
            .map(|c| c.channel().id)
            .collect();
        let mut seen = std::collections::HashSet::new();
        let new: Vec<pcore::Channel> = channels
            .into_iter()
            .filter(|c| !c.id.id.is_empty() && !known.contains(&c.id) && seen.insert(c.id.clone()))
            .collect();
        let added = new.len();
        self.import_channels(new);
        log::info!("Imported {added} of {total} subscriptions from the YouTube account");
        Ok((added, total))
    }

    /// YouTube's "Up next" for a video, personalised when an account is set up.
    pub async fn up_next(&self, video: &Video) -> Vec<Video> {
        if video.platform() != "youtube" {
            return Vec::new();
        }
        let (core, account) = (self.core(), self.account_config());
        let id = video.id();
        let result = tspawn!(async move { core.up_next(&account, &id).await })
            .await
            .expect("Failed to spawn tokio");

        match result {
            Ok(videos) => {
                log::info!("Up next: {} videos for {}", videos.len(), video.id().id);
                let videos: Vec<Video> = videos
                    .into_iter()
                    .map(|v| self.construct_video(v))
                    .collect();
                // YouTube's newer layout leaves out the channel and date; fetch them.
                self.fill_in_details_later(videos.clone());
                videos
            }
            Err(e) => {
                log::error!("Failed to load Up next: {e}");
                Vec::new()
            }
        }
    }

    /// Kinora: fast start. A URI Clapper can play right away for a YouTube
    /// video (see `kinora_core::streams`), or `None` when the caller should fall back to
    /// the yt-dlp path (setting off, other platform, or the lookup failed).
    pub async fn fast_playback_uri(&self, video: &Video) -> Option<String> {
        if video.platform() != "youtube" || !self.playback_config().fast_start {
            return None;
        }
        let core = self.core();
        let id = video.id();
        let started = std::time::Instant::now();
        let result = tspawn!(async move { core.fast_playback_uri(&id).await })
            .await
            .expect("Failed to spawn tokio");
        match result {
            Ok(uri) => {
                log::info!(
                    "Fast start: streams for {} in {:.2} s",
                    video.id().id,
                    started.elapsed().as_secs_f64()
                );
                Some(uri)
            }
            Err(e) => {
                log::warn!("Fast start failed for {}, using yt-dlp: {e}", video.id().id);
                None
            }
        }
    }

    /// Kinora: look a video's streams up ahead of time (e.g. the first "Up next"
    /// video), so it starts at once when picked or autoplayed.
    pub fn prepare_playback(&self, video: &Video) {
        if video.platform() != "youtube" {
            return;
        }
        let (core, playback) = (self.core(), self.playback_config());
        let id = video.id();
        tspawn!(async move {
            if let Err(e) = core.prepare(&playback, &id).await {
                // The fast-start lookup failing is expected while it is experimental.
                if playback.fast_start {
                    log::debug!("Could not prepare {}: {e}", id.id);
                } else {
                    log::warn!("Could not prepare {}: {e}", id.id);
                }
            }
        });
    }

    /// Kinora: a prepared playback URI for this video (see `prepare_playback`).
    pub fn prepared_playback_uri(&self, video: &Video) -> Option<String> {
        self.core().prepared_uri(&video.id())
    }

    // ---- Kinora: the DJ (see `gui/dj.rs` and docs/architecture.md, "The DJ") ----

    /// Whether the DJ extension is switched on.
    pub fn dj_enabled(&self) -> bool {
        self.core()
            .extensions()
            .is_enabled(kinora_core::extension::builtin::dj::ID)
    }

    /// Where a DJ session starts: the given video's radio, or "my mix",
    /// following the DJ's "start from" setting.
    pub fn dj_seed(&self, video: Option<&Video>) -> kinora_core::dj::DjSeed {
        let id = video.map(|v| v.id());
        self.core().dj_seed(&self.account_config(), id.as_ref())
    }

    /// Start a DJ session (one request to YouTube Music).
    pub async fn dj_start(
        &self,
        seed: kinora_core::dj::DjSeed,
    ) -> Result<kinora_core::dj::DjSession, String> {
        let (core, account) = (self.core(), self.account_config());
        tspawn!(async move {
            core.dj_start(&account, seed)
                .await
                .map_err(|e| e.to_string())
        })
        .await
        .expect("Failed to spawn tokio")
    }

    /// Fetch more tracks for a session: the answer to its `refill_request()`.
    pub async fn dj_more(
        &self,
        request: kinora_core::dj::RadioRequest,
    ) -> Result<kinora_core::dj::RadioPage, String> {
        let (core, account) = (self.core(), self.account_config());
        tspawn!(async move {
            core.dj_more(&account, &request)
                .await
                .map_err(|e| e.to_string())
        })
        .await
        .expect("Failed to spawn tokio")
    }

    /// How the track that plays (its real length and position) gives way to `next`.
    pub fn dj_transition(
        &self,
        duration: Option<std::time::Duration>,
        position: std::time::Duration,
        next: &kinora_core::dj::Track,
    ) -> kinora_core::dj::TransitionPlan {
        self.core().dj_transition(duration, position, next)
    }

    /// The video model of a DJ track (shared with the rest of the app).
    pub fn dj_video(&self, track: &kinora_core::dj::Track) -> Video {
        self.construct_video(track.to_video())
    }

    /// The video has really been watched: tell the extensions (in the
    /// background). The account extension marks it watched on YouTube.
    pub fn mark_watched_on_account(&self, video: &Video) {
        if video.platform() != "youtube" {
            return;
        }
        let (core, account) = (self.core(), self.account_config());
        let video = video.video();
        tspawn!(async move { core.watched(&account, &video).await });
    }

    pub fn toggle_subscription(&self, channel: Channel) {
        self.toggle_subscription_internal(channel, true);
    }

    fn toggle_subscription_internal(&self, channel: Channel, interact_with_store: bool) {
        let now_subscribed = self.subscriptions_store().toggle(channel.clone());
        channel.set_subscribed(now_subscribed);

        if interact_with_store {
            let mut binding = self.imp().store.borrow_mut();
            let store = binding.as_mut().expect("Store to be set up");
            // TODO: Error handling?
            if now_subscribed {
                let _ = store.subscribe(&channel.channel());
            } else {
                let _ = store.unsubscribe(&channel.channel());
            }
        }
    }

    pub fn toggle_watch_later(&self, video: Video) {
        self.toggle_watch_later_internal(video, true);
    }

    fn toggle_watch_later_internal(&self, video: Video, interact_with_store: bool) {
        let now_watch_later = self.watch_later_store().toggle(video.clone());
        video.set_watch_later(now_watch_later);

        if interact_with_store {
            let mut binding = self.imp().store.borrow_mut();
            let store = binding.as_mut().expect("Store to be set up");
            // TODO: Error handling?
            if now_watch_later {
                let _ = store.watch_later(&video.video());
            } else {
                let _ = store.remove_watch_later(&video.video());
            }
        }
    }

    pub fn toggle_filter(&self, filter: Filter) {
        self.toggle_filter_internal(filter, true);
    }

    fn toggle_filter_internal(&self, filter: Filter, interact_with_store: bool) {
        let now_active = self.filter_store().toggle(filter.clone());

        if interact_with_store {
            let mut binding = self.imp().store.borrow_mut();
            let store = binding.as_mut().expect("Store to be set up");
            // TODO: Error handling?
            if now_active {
                let _ = store.add_filter(&filter);
            } else {
                let _ = store.remove_filter(&filter);
            }
        }
    }

    pub fn subscriptions(&self) -> Vec<Channel> {
        self.subscriptions_store().channels()
    }

    pub fn watched(&self, video: Video, timestamp: DateTime<Utc>) {
        if self.imp().settings.boolean("watch-history-enabled") {
            self.watched_internal(video, timestamp, true);
        }
    }

    fn watched_internal(&self, video: Video, timestamp: DateTime<Utc>, interact_with_store: bool) {
        self.watched_store().remove(video.clone());
        video.set_watched_at(Some(timestamp));
        self.watched_store().add(video.clone());

        self.filter_store().changed(gtk::FilterChange::MoreStrict);

        if interact_with_store {
            let mut binding = self.imp().store.borrow_mut();
            let store = binding.as_mut().expect("Store to be set up");
            // TODO: Error handling?
            let _ = store.watched(&video.video(), &timestamp);
        }
    }

    pub fn toggle_watched(&self, video: Video, timestamp: DateTime<Utc>) {
        if self.imp().settings.boolean("watch-history-enabled") {
            self.toggle_watched_internal(video, timestamp, true);
        }
    }

    fn toggle_watched_internal(
        &self,
        video: Video,
        timestamp: DateTime<Utc>,
        interact_with_store: bool,
    ) {
        let watched = self.watched_store().toggle(video.clone());
        if watched {
            self.watched_store().remove(video.clone());
            video.set_watched_at(Some(timestamp));
            self.watched_store().add(video.clone());
        } else {
            video.set_watched_at(None);
        }

        self.filter_store().changed(gtk::FilterChange::Different);

        if interact_with_store {
            let mut binding = self.imp().store.borrow_mut();
            let store = binding.as_mut().expect("Store to be set up");
            // TODO: Error handling?
            if watched {
                let _ = store.watched(&video.video(), &timestamp);
            } else {
                let _ = store.unwatched(&video.video());
            }
        }
    }

    pub fn delete_watch_history(&self) {
        // Delete from store.
        let mut binding = self.imp().store.borrow_mut();
        let store = binding.as_mut().expect("Store to be set up");
        // TODO: Error handling?
        let _ = store.delete_watch_history();

        let watched_store = self.watched_store();

        // Set watched to false for all videos in the in-memory cache.
        for video in self.imp().watched_store.borrow().iter::<Video>().flatten() {
            video.set_watched_at(None);
        }

        // Clear the in-memory watched cache.
        watched_store.clear();

        // Notify the filter that it changed.
        self.filter_store().changed(gtk::FilterChange::LessStrict);
    }

    pub fn has_watch_history(&self) -> bool {
        !self.imp().watched_store.borrow().is_empty()
    }

    pub fn downloaded(&self, video: Video, to: PathBuf) {
        self.downloaded_internal(video, to, true);
    }

    pub fn downloaded_internal(&self, video: Video, to: PathBuf, interact_with_store: bool) {
        // TODO: Make some "add".
        self.downloaded_store().toggle(video.clone());
        video.set_downloaded_path(Some(to.to_string_lossy()));

        if interact_with_store {
            let mut binding = self.imp().store.borrow_mut();
            let store = binding.as_mut().expect("Store to be set up");
            // TODO: Error handling?
            let _ = store.downloaded(&video.id(), &to);
        }
    }

    pub fn remove_downloaded(&self, video: &Video) {
        // TODO: Make some "remove".
        self.downloaded_store().toggle(video.clone());
        video.set_downloaded_path(None::<String>);

        let mut binding = self.imp().store.borrow_mut();
        let store = binding.as_mut().expect("Store to be set up");
        // TODO: Error handling?
        let _ = store.remove_downloaded(&video.id());
    }

    pub fn import_youtube(&self, file: File) -> Result<(), Box<dyn std::error::Error>> {
        let channels = import::from_youtube_csv(file)?;
        self.import_channels(channels);

        Ok(())
    }

    pub fn import_invidious(&self, file: File) -> Result<(), Box<dyn std::error::Error>> {
        let channels = import::from_invidious_json(file)?;
        self.import_channels(channels);

        Ok(())
    }

    pub fn import_newpipe(&self, file: File) -> Result<(), Box<dyn std::error::Error>> {
        let channels = import::from_newpipe_json(file)?;
        self.import_channels(channels);

        Ok(())
    }

    pub fn export_newpipe(&self, file: File) -> Result<(), Box<dyn std::error::Error>> {
        let channels = self.subscriptions();
        export::to_newpipe_json(
            &channels.iter().map(|c| c.channel()).collect::<Vec<_>>(),
            file,
        )?;
        Ok(())
    }

    pub fn export_youtube(&self, file: File) -> Result<(), Box<dyn std::error::Error>> {
        let channels = self.subscriptions();
        export::to_youtube_csv(
            &channels.iter().map(|c| c.channel()).collect::<Vec<_>>(),
            file,
        )?;
        Ok(())
    }

    fn import_channels(&self, channels: Vec<pcore::Channel>) {
        for channel in channels {
            let channel = self.construct_channel(channel);
            self.subscriptions_store()
                .add_if_not_exists(channel.clone());
            channel.set_subscribed(true);

            let mut binding = self.imp().store.borrow_mut();
            let store = binding.as_mut().expect("Store to be set up");
            let _ = store.subscribe(&channel.channel());
        }
    }

    async fn await_setup_finished(&self) {
        let mut receiver = self.imp().setup_finished_watch_channel.subscribe();
        receiver
            .wait_for(|b| *b)
            .await
            .expect("Failed to await setup finished");
    }
}

mod imp {
    use gdk::gio::Settings;
    use gdk::prelude::ObjectExt;
    use gdk::subclass::prelude::{DerivedObjectProperties, ObjectImpl, ObjectSubclass};
    use glib::{Properties, WeakRef};
    use gtk::glib;
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        sync::Arc,
    };
    use tokio::sync::Mutex;

    use crate::backend::{Channel, FilterStore, FiniteVideoStore, SubscriptionsStore, Video};
    use crate::store::{Cache, Store};

    #[derive(Properties)]
    #[properties(wrapper_type = super::Client)]
    pub struct Client {
        pub(super) internal: Arc<Mutex<papi::PipelineApi>>,
        pub(super) store: RefCell<Option<Store>>,
        pub(super) cache: RefCell<Option<Cache>>,

        pub(super) reqwest_client: RefCell<Option<reqwest::Client>>,

        pub(super) channel_cache: RefCell<HashMap<pcore::ChannelId, WeakRef<Channel>>>,
        pub(super) video_cache: RefCell<HashMap<pcore::VideoId, WeakRef<Video>>>,
        pub(super) title_cache: RefCell<HashMap<pcore::VideoId, String>>,

        #[property(get, set)]
        pub(super) subscriptions_store: RefCell<SubscriptionsStore>,
        #[property(get, set)]
        pub(super) watch_later_store: RefCell<FiniteVideoStore>,
        #[property(get, set)]
        pub(super) downloaded_store: RefCell<FiniteVideoStore>,
        #[property(get, set)]
        pub(super) watched_store: RefCell<FiniteVideoStore>,
        #[property(get, set)]
        pub(super) filter_store: RefCell<FilterStore>,

        // Kinora
        /// The UI-independent core: Home, Up next, the account, playback preparation.
        pub(super) core: RefCell<Option<Arc<kinora_core::Core>>>,
        #[property(get, set)]
        pub(super) home_store: RefCell<FiniteVideoStore>,
        #[property(get, set)]
        pub(super) home_loading: Cell<bool>,
        #[property(get, set)]
        pub(super) home_error: RefCell<String>,
        #[property(get, set)]
        pub(super) has_account: Cell<bool>,
        #[property(get, set)]
        pub(super) account_label: RefCell<String>,
        #[property(get, set)]
        pub(super) importing_subscriptions: Cell<bool>,
        /// The Home filter chips' titles (`StringObject`s); empty without chips.
        #[property(get)]
        pub(super) home_chips: RefCell<gdk::gio::ListStore>,
        /// Index of the chip whose feed Home shows.
        #[property(get, set)]
        pub(super) home_chip_selected: Cell<u32>,
        /// Continuation tokens of the chips, in the same order (`None` for "All").
        pub(super) home_chip_tokens: RefCell<Vec<Option<String>>>,
        /// Kinora: playlist and mix cards that came with the Home page.
        pub(super) home_playlists: RefCell<Vec<kinora_core::PlaylistCard>>,
        /// Continuation that loads the next page of Home.
        pub(super) home_next: RefCell<Option<String>>,
        /// Counts Home loads, so a superseded one can be ignored.
        pub(super) home_gen: Cell<u32>,
        pub(super) home_loading_more: Cell<bool>,

        pub(super) setup_finished_watch_channel: tokio::sync::watch::Sender<bool>,

        pub(super) settings: Settings,
    }

    impl Default for Client {
        fn default() -> Self {
            Self {
                internal: Default::default(),
                store: Default::default(),
                cache: Default::default(),
                reqwest_client: Default::default(),
                channel_cache: Default::default(),
                video_cache: Default::default(),
                title_cache: Default::default(),
                subscriptions_store: Default::default(),
                watch_later_store: Default::default(),
                downloaded_store: Default::default(),
                filter_store: Default::default(),
                core: Default::default(),
                home_store: RefCell::new(FiniteVideoStore::with_sort(
                    crate::backend::finite_video_store::VideoSort::AsAdded,
                )),
                home_loading: Default::default(),
                home_error: Default::default(),
                has_account: Default::default(),
                account_label: Default::default(),
                importing_subscriptions: Default::default(),
                home_chips: RefCell::new(gdk::gio::ListStore::new::<gtk::StringObject>()),
                home_chip_selected: Default::default(),
                home_chip_tokens: Default::default(),
                home_playlists: Default::default(),
                home_next: Default::default(),
                home_gen: Default::default(),
                home_loading_more: Default::default(),
                watched_store: RefCell::new(FiniteVideoStore::with_sort(
                    crate::backend::finite_video_store::VideoSort::WatchedDescending,
                )),
                setup_finished_watch_channel: tokio::sync::watch::channel(false).0,
                settings: Settings::new(crate::config::BASE_ID),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Client {
        const NAME: &'static str = "TFClient";
        type Type = super::Client;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Client {}
}

/// Kinora: how the connected YouTube account is shown.
fn account_label(settings: &gdk::gio::Settings) -> String {
    let label = settings.string("account-label");
    if label.is_empty() {
        gettextrs::gettext("YouTube account")
    } else {
        label.to_string()
    }
}

/// Kinora: whether a search stream event belongs to the given platform.
fn search_result_on(result: &pcore::SearchStreamResult, platform: &str) -> bool {
    match result {
        pcore::SearchStreamResult::Loading(p)
        | pcore::SearchStreamResult::Loaded(p)
        | pcore::SearchStreamResult::ErrorLoading(p, _) => p == platform,
        pcore::SearchStreamResult::NextItem(pcore::SearchItem::Video(v)) => {
            v.id.platform == platform
        }
        pcore::SearchStreamResult::NextItem(pcore::SearchItem::Channel(c)) => {
            c.id.platform == platform
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_filter_matches_platform_events() {
        let loading = pcore::SearchStreamResult::Loading("youtube".into());
        assert!(search_result_on(&loading, "youtube"));
        assert!(!search_result_on(&loading, "peertube"));

        let channel =
            pcore::SearchStreamResult::NextItem(pcore::SearchItem::Channel(pcore::Channel {
                id: ChannelId {
                    id: "x".into(),
                    platform: "peertube".into(),
                },
                name: "x".into(),
                avatar: None,
                banner: None,
                description: None,
                subscribers: None,
            }));
        assert!(search_result_on(&channel, "peertube"));
        assert!(!search_result_on(&channel, "youtube"));
    }
}
