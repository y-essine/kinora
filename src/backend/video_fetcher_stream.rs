use std::cell::RefCell;
use std::pin::Pin;

use futures::StreamExt;
use gdk::prelude::{Cast, ListModelExt};
use gdk::subclass::prelude::ObjectSubclassIsExt;
use gdk::{gio, glib::Object};
use glib::object::ObjectExt;
use glib::{BoxedAnyObject, clone};
use gtk::prelude::FilterExt;

use crate::backend::FilterStore;
use crate::{gspawn, tspawn};

use super::Client;

// TODO: Section model?
gtk::glib::wrapper! {
    pub struct VideoFetcherStream(ObjectSubclass<imp::VideoFetcherStream>)
        @implements gio::ListModel, gtk::SelectionModel;
}

impl Default for VideoFetcherStream {
    fn default() -> Self {
        Self::empty()
    }
}

impl VideoFetcherStream {
    pub fn new(
        stream: Pin<Box<dyn pcore::VideoFetcherStream>>,
        client: &Client,
        filter: &FilterStore,
    ) -> Self {
        let s: Self = Object::builder()
            .property("client", client)
            .property("filter", filter)
            .build();
        s.imp()
            .stream
            .try_lock()
            .expect("mutex to not be poisoned")
            .replace(stream);

        filter.connect_changed(clone!(
            #[weak]
            s,
            move |_, _| {
                s.reevaluate_filter();
            }
        ));

        s
    }

    pub fn empty() -> Self {
        Self::new(
            Box::pin(futures::stream::empty()),
            &Client::new(),
            &FilterStore::default(),
        )
    }

    pub fn load_more(&self, num_videos: usize) {
        gspawn!(clone!(
            #[weak(rename_to = s)]
            self,
            async move { s.load_more_async(num_videos).await }
        ));
    }

    fn reevaluate_filter(&self) {
        let all_videos = self.imp().all_videos.borrow();
        let filter = self.filter();
        let next: Vec<_> = all_videos
            .iter()
            .filter(|&v| filter.match_(v))
            .cloned()
            .collect();

        let next_len = next.len();

        let prev = self.imp().filtered_videos.replace(next);
        let prev_len = prev.len();

        self.items_changed(0, prev_len as u32, next_len as u32);
    }

    async fn load_more_async(&self, num_videos: usize) {
        let client = self.client();
        let filter = self.filter();
        let stream = self.imp().stream.clone();

        // Quick return if already loading.
        // Should not be a race condition as everything happens on the GTK thread and there is no async.
        if self.loading() {
            return;
        }
        self.set_loading(true);

        let mut all_loaded = Vec::with_capacity(num_videos);
        let mut filtered_loaded = Vec::with_capacity(num_videos);
        while let Some(next_item) = {
            let binding = stream.clone();
            tspawn!(async move {
                let mut stream = binding.try_lock().ok()?;
                let stream = stream.as_mut().expect("stream to be set up");

                stream.next().await
            })
            .await
            .expect("Failed to spawn tokio")
        } {
            match next_item {
                pcore::VideoFetcherStreamResult::Loading(new_channels) => {
                    let num_channels = new_channels.len();
                    log::trace!("Loading {num_channels} channels");
                    let mut channels = self.imp().channels.borrow_mut();
                    let mut loaded = self.imp().loaded.borrow_mut();

                    for c in new_channels {
                        loaded.remove(&c);
                        channels.insert(c);
                    }

                    drop(channels);
                    drop(loaded);
                    self.notify("loaded-fraction");
                }
                pcore::VideoFetcherStreamResult::Loaded(new_channels) => {
                    let num_channels = new_channels.len();
                    log::trace!("Loaded {num_channels} channels");
                    let mut loaded = self.imp().loaded.borrow_mut();
                    loaded.extend(new_channels);
                    drop(loaded);
                    self.notify("loaded-fraction");
                }
                pcore::VideoFetcherStreamResult::ErrorLoading(new_channels, error) => {
                    let num_channels = new_channels.len();
                    log::trace!("Error on {num_channels} channels");
                    let mut error_loading = self.imp().error_loading.borrow_mut();
                    error_loading.extend(new_channels);
                    drop(error_loading);
                    self.notify("loaded-fraction");

                    self.emit_by_name(
                        "error",
                        &[&BoxedAnyObject::new((
                            gettextrs::ngettext(
                                "Error loading one channel",
                                "Error loading {} channels",
                                num_channels as u32,
                            )
                            .replace("{}", &num_channels.to_string()),
                            RefCell::new(Some(error)), // This is a pretty complex type as we need ownership of the error (not just a reference), which is not easily possible with `BoxedAnyObject`.
                        ))],
                    )
                }
                pcore::VideoFetcherStreamResult::NextVideo(v) => {
                    let video = client.construct_video(v);
                    all_loaded.push(video.clone());
                    if filter.match_(&video) {
                        filtered_loaded.push(video);
                    }
                }
            }
            if filtered_loaded.len() == num_videos {
                break;
            }
        }

        self.set_finished(filtered_loaded.len() != num_videos);

        let mut all_videos = self.imp().all_videos.borrow_mut();
        all_videos.extend(all_loaded);

        let num_filtered_loaded = filtered_loaded.len();
        let mut filtered_videos = self.imp().filtered_videos.borrow_mut();
        let previous_length = filtered_videos.len();
        filtered_videos.extend(filtered_loaded);

        drop(all_videos);
        drop(filtered_videos);

        self.items_changed(previous_length as u32, 0, num_filtered_loaded as u32);

        self.set_loading(false);
        self.notify_is_empty();
    }

    fn items_changed(&self, position: u32, removed: u32, added: u32) {
        self.upcast_ref::<gio::ListModel>()
            .items_changed(position, removed, added);
    }
}

mod imp {
    use gdk::gio;
    use gdk::prelude::Cast;
    use glib::BoxedAnyObject;
    use glib::subclass::Signal;
    use glib::subclass::types::ObjectSubclassExt;
    use glib::types::StaticType;
    use gtk::glib;
    use gtk::prelude::SelectionModelExt;
    use gtk::subclass::selection_model::SelectionModelImpl;
    use pcore::ChannelId;
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;
    use std::marker::PhantomData;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::LazyLock;
    use tokio::sync::Mutex;

    use gdk::subclass::prelude::{DerivedObjectProperties, ListModelImpl};
    use gdk::{
        glib::Properties,
        prelude::ObjectExt,
        subclass::prelude::{ObjectImpl, ObjectSubclass},
    };

    use crate::backend::{Client, FilterStore, Video};

    type InnerStream = Arc<Mutex<Option<Pin<Box<dyn pcore::VideoFetcherStream>>>>>;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::VideoFetcherStream)]
    pub struct VideoFetcherStream {
        pub(super) stream: InnerStream,

        pub(super) all_videos: RefCell<Vec<Video>>,
        #[property(name = "is-empty", type = bool, get = |s: &Self| s.filtered_videos.borrow().is_empty())]
        pub(super) filtered_videos: RefCell<Vec<Video>>,

        #[property(get = Self::get_loaded_fraction)]
        loaded_fraction: PhantomData<f64>,
        pub(super) loaded: RefCell<HashSet<ChannelId>>,
        pub(super) channels: RefCell<HashSet<ChannelId>>,
        pub(super) error_loading: RefCell<HashSet<ChannelId>>,

        #[property(get, set)]
        finished: Cell<bool>,

        #[property(get, set)]
        loading: Cell<bool>,

        #[property(get, set = Self::set_selected, nullable)]
        selected: RefCell<Option<Video>>,

        // TODO: Errors
        #[property(get, set, construct_only)]
        client: RefCell<Client>,
        #[property(get, set, construct_only)]
        filter: RefCell<FilterStore>,
    }

    impl VideoFetcherStream {
        fn set_selected(&self, selected: Option<Video>) {
            self.selected.replace(selected);
            // Don't know where exactly it changed; emit that it changed everywhere.
            let num_videos = self.filtered_videos.borrow().len() as u32;
            if num_videos > 0 {
                self.obj().selection_changed(0, num_videos);
            }
        }

        fn get_loaded_fraction(&self) -> f64 {
            let loaded = self.loaded.borrow().len() + self.error_loading.borrow().len();
            let channels = self.channels.borrow().len();

            if channels == 0 {
                0.0
            } else {
                (loaded as f64) / (channels as f64)
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VideoFetcherStream {
        const NAME: &'static str = "TFVideoFetcherStream";
        type Type = super::VideoFetcherStream;
        type Interfaces = (gio::ListModel, gtk::SelectionModel);
    }

    #[glib::derived_properties]
    impl ObjectImpl for VideoFetcherStream {
        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("error")
                        .param_types([BoxedAnyObject::static_type()])
                        .build(),
                ]
            });
            SIGNALS.as_ref()
        }
    }

    impl ListModelImpl for VideoFetcherStream {
        fn item_type(&self) -> glib::Type {
            Video::static_type()
        }

        fn n_items(&self) -> u32 {
            self.filtered_videos
                .borrow()
                .len()
                .try_into()
                .unwrap_or_default()
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let list = self.filtered_videos.borrow();

            list.get(position as usize)
                .map(|o| o.clone().upcast::<glib::Object>())
        }
    }

    impl SelectionModelImpl for VideoFetcherStream {
        fn is_selected(&self, position: u32) -> bool {
            let list = self.filtered_videos.borrow();
            let selection = self.selected.borrow();
            let Some(selection) = selection.as_ref() else {
                return false;
            };
            list.get(position as usize)
                .is_some_and(|v| v.id() == selection.id())
        }

        fn selection_in_range(&self, position: u32, n_items: u32) -> gtk::Bitset {
            let result = gtk::Bitset::new_range(position, n_items);
            let list = self.filtered_videos.borrow();
            let selection = self.selected.borrow();
            let Some(selection) = selection.as_ref() else {
                return result;
            };

            let index: Option<u32> = list
                .iter()
                .position(|v| v.id() == selection.id())
                .and_then(|v| v.try_into().ok());

            if let Some(index) = index
                && position <= index
                && index < position + n_items
            {
                result.add(position);
            }
            result
        }
    }
}
