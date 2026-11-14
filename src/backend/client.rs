use std::path::PathBuf;

use chrono::{DateTime, Utc};
use gdk::gdk_pixbuf::Pixbuf;
use gdk::gio::prelude::{FileExt, ListModelExtManual};
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
        sqlite_path.push("tubefeeder");
        sqlite_path.push("data.sqlite");
        let store = Store::new(sqlite_path)?;
        self.imp().store.borrow_mut().replace(store);
        self.load_data()?;

        let mut cache_path = glib::user_cache_dir();
        cache_path.push("tubefeeder");
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
            rustypipe_path.push("tubefeeder");
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

    pub async fn search(&self, query: &str) -> SearchStream {
        self.await_setup_finished().await;
        SearchStream::new(self.internal().await.search(query), self)
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

    // ---- Pipeline Home: the signed-in YouTube account ----

    fn account_cookies_command(&self) -> String {
        self.imp().settings.string("account-cookies-command").to_string()
    }

    /// Reload the Home page from the account's YouTube home feed.
    pub async fn refresh_home(&self) {
        if self.home_loading() {
            return;
        }
        self.set_home_loading(true);
        self.set_home_error(String::new());

        let command = self.account_cookies_command();
        let result = tspawn!(async move {
            let cookies = super::account::cookies_file(&command)
                .await?
                .ok_or("No YouTube account is set up. Set the account cookies command in the preferences.")?;
            super::account::home_feed(&cookies).await
        })
        .await
        .expect("Failed to spawn tokio");

        let videos: Vec<Video> = match result {
            Ok(videos) => videos.into_iter().map(|v| self.construct_video(v)).collect(),
            Err(e) => {
                log::error!("Failed to load the home feed: {e}");
                self.set_home_error(e.to_string());
                self.set_home_loading(false);
                return;
            }
        };

        let store = self.home_store();
        store.clear();
        for video in &videos {
            store.add(video.clone());
        }
        if videos.is_empty() {
            self.set_home_error(
                "YouTube returned no recommendations. Watch a few videos on the account first."
                    .to_owned(),
            );
        }
        self.set_home_loading(false);

        // The listing has no upload dates or view counts; fill them in. That can
        // swap the thumbnail URL, which clears the loaded thumbnail, so reload it.
        for video in videos {
            if let Err(e) = self.populate_video_information(&video.id()).await {
                log::debug!("Home: no extra information for {}: {e}", video.id().id);
            }
            self.populate_thumbnail(&video).await;
        }
    }

    /// YouTube's "Up next" for a video, personalised when an account is set up.
    pub async fn up_next(&self, video: &Video) -> Vec<Video> {
        if video.platform() != "youtube" {
            return Vec::new();
        }
        let command = self.account_cookies_command();
        let mut storage_dir = glib::user_cache_dir();
        storage_dir.push("tubefeeder");
        storage_dir.push("account");
        let id = video.id().id;

        let result = tspawn!(async move {
            let cookies = super::account::cookies_file(&command)
                .await
                .unwrap_or_else(|e| {
                    log::warn!("Up next: {e}");
                    None
                });
            super::account::up_next(storage_dir, cookies, id).await
        })
        .await
        .expect("Failed to spawn tokio");

        match result {
            Ok(videos) => videos.into_iter().map(|v| self.construct_video(v)).collect(),
            Err(e) => {
                log::error!("Failed to load Up next: {e}");
                Vec::new()
            }
        }
    }

    /// Mark a video as watched on the YouTube account (in the background).
    pub fn mark_watched_on_account(&self, video: &Video) {
        if video.platform() != "youtube" {
            return;
        }
        let command = self.account_cookies_command();
        let id = video.id().id;
        tspawn!(async move {
            let result = async {
                if let Some(cookies) = super::account::cookies_file(&command).await? {
                    super::account::mark_watched(&cookies, &id).await?;
                    log::info!("Marked {id} watched on the YouTube account");
                }
                Ok::<_, super::account::Error>(())
            }
            .await;
            if let Err(e) = result {
                log::warn!("Could not mark {id} watched on the YouTube account: {e}");
            }
        });
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

        // Pipeline Home
        #[property(get, set)]
        pub(super) home_store: RefCell<FiniteVideoStore>,
        #[property(get, set)]
        pub(super) home_loading: Cell<bool>,
        #[property(get, set)]
        pub(super) home_error: RefCell<String>,

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
                home_store: RefCell::new(FiniteVideoStore::with_sort(
                    crate::backend::finite_video_store::VideoSort::AsAdded,
                )),
                home_loading: Default::default(),
                home_error: Default::default(),
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
