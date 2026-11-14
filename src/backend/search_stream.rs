use std::cell::RefCell;
use std::pin::Pin;

use futures::StreamExt;
use gdk::prelude::{Cast, ListModelExt};
use gdk::subclass::prelude::ObjectSubclassIsExt;
use gdk::{gio, glib::Object};
use glib::object::ObjectExt;
use glib::{BoxedAnyObject, clone};

use crate::{gspawn, tspawn};

use super::Client;

// TODO: Section model?
gtk::glib::wrapper! {
    pub struct SearchStream(ObjectSubclass<imp::SearchStream>)
        @implements gio::ListModel, gtk::SelectionModel;
}

// TODO: Consider filtering?

impl SearchStream {
    pub fn new(stream: Pin<Box<dyn pcore::SearchStream>>, client: &Client) -> Self {
        let s: Self = Object::builder().property("client", client).build();
        s.imp()
            .stream
            .try_lock()
            .expect("mutex to not be poisoned")
            .replace(stream);

        s
    }

    pub fn empty() -> Self {
        Self::new(Box::pin(futures::stream::empty()), &Client::new())
    }

    pub fn load_more(&self, num_items: usize) {
        gspawn!(clone!(
            #[weak(rename_to = s)]
            self,
            async move { s.load_more_async(num_items).await }
        ));
    }

    async fn load_more_async(&self, num_items: usize) {
        let client = self.client();
        let stream = self.imp().stream.clone();

        // Quick return if already loading.
        // Should not be a race condition as everything happens on the GTK thread and there is no async.
        if self.loading() {
            return;
        }
        self.set_loading(true);

        let mut new_loaded = Vec::with_capacity(num_items);
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
                pcore::SearchStreamResult::Loading(platform) => {
                    log::trace!("Loading {platform}");
                    let mut platforms = self.imp().platforms.borrow_mut();
                    let mut loaded = self.imp().loaded.borrow_mut();

                    loaded.remove(&platform);
                    platforms.insert(platform);

                    drop(platforms);
                    drop(loaded);
                    self.notify("loaded-fraction");
                }
                pcore::SearchStreamResult::Loaded(platform) => {
                    log::trace!("Loaded {platform}");
                    let mut loaded = self.imp().loaded.borrow_mut();
                    loaded.insert(platform);
                    drop(loaded);
                    self.notify("loaded-fraction");
                }
                pcore::SearchStreamResult::ErrorLoading(platform, error) => {
                    log::trace!("Error loading {platform} ");
                    let mut error_loading = self.imp().error_loading.borrow_mut();
                    error_loading.insert(platform);
                    drop(error_loading);
                    self.notify("loaded-fraction");

                    self.emit_by_name(
                        "error",
                        &[&BoxedAnyObject::new((
                            gettextrs::gettext("Error searching one platform"),
                            RefCell::new(Some(error)), // This is a pretty complex type as we need ownership of the error (not just a reference), which is not easily possible with `BoxedAnyObject`.
                        ))],
                    )
                }
                pcore::SearchStreamResult::NextItem(v) => {
                    let item = client.construct_search_item(v);
                    new_loaded.push(item.clone());
                }
            }
            if new_loaded.len() == num_items {
                break;
            }
        }

        self.set_finished(new_loaded.len() != num_items);

        let mut items = self.imp().items.borrow_mut();

        let num_loaded = new_loaded.len();
        let previous_length = items.len();

        items.extend(new_loaded);

        drop(items);

        self.items_changed(previous_length as u32, 0, num_loaded as u32);

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
    use glib::object::CastNone;
    use glib::subclass::Signal;
    use glib::subclass::types::ObjectSubclassExt;
    use glib::types::StaticType;
    use glib::{BoxedAnyObject, Object};
    use gtk::glib;
    use gtk::prelude::SelectionModelExt;
    use gtk::subclass::selection_model::SelectionModelImpl;
    use pcore::PlatformId;
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

    use crate::backend::{Channel, Client, SearchItem, Video};

    type InnerStream = Arc<Mutex<Option<Pin<Box<dyn pcore::SearchStream>>>>>;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::SearchStream)]
    pub struct SearchStream {
        pub(super) stream: InnerStream,

        #[property(name = "is-empty", type = bool, get = |s: &Self| s.items.borrow().is_empty())]
        pub(super) items: RefCell<Vec<SearchItem>>,

        #[property(get = Self::get_loaded_fraction)]
        loaded_fraction: PhantomData<f64>,
        pub(super) loaded: RefCell<HashSet<PlatformId>>,
        pub(super) platforms: RefCell<HashSet<PlatformId>>,
        pub(super) error_loading: RefCell<HashSet<PlatformId>>,

        #[property(get, set)]
        finished: Cell<bool>,

        #[property(get, set)]
        loading: Cell<bool>,

        #[property(get, set = Self::set_selected, nullable)]
        selected: RefCell<Option<SearchItem>>,

        // TODO: Errors
        #[property(get, set, construct_only)]
        client: RefCell<Client>,
    }

    impl SearchStream {
        // More readable as we have more than three cases.
        #[allow(clippy::manual_map)]
        fn set_selected(&self, selected: Option<Object>) {
            let search_item = if let Some(video) = selected.and_dynamic_cast_ref::<Video>() {
                Some(SearchItem::for_video(video))
            } else if let Some(channel) = selected.and_dynamic_cast_ref::<Channel>() {
                Some(SearchItem::for_channel(channel))
            } else {
                None
            };
            self.selected.replace(search_item);
            // Don't know where exactly it changed; emit that it changed everywhere.
            let num_items = self.items.borrow().len() as u32;
            if num_items > 0 {
                self.obj().selection_changed(0, num_items);
            }
        }

        fn get_loaded_fraction(&self) -> f64 {
            let loaded = self.loaded.borrow().len() + self.error_loading.borrow().len();
            let platforms = self.platforms.borrow().len();

            if platforms == 0 {
                0.0
            } else {
                (loaded as f64) / (platforms as f64)
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SearchStream {
        const NAME: &'static str = "TFSearchStream";
        type Type = super::SearchStream;
        type Interfaces = (gio::ListModel, gtk::SelectionModel);
    }

    #[glib::derived_properties]
    impl ObjectImpl for SearchStream {
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

    impl ListModelImpl for SearchStream {
        fn item_type(&self) -> glib::Type {
            SearchItem::static_type()
        }

        fn n_items(&self) -> u32 {
            self.items.borrow().len().try_into().unwrap_or_default()
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let list = self.items.borrow();

            list.get(position as usize)
                .map(|o| o.clone().upcast::<glib::Object>())
        }
    }

    impl SelectionModelImpl for SearchStream {
        fn is_selected(&self, position: u32) -> bool {
            let list = self.items.borrow();
            let selection = self.selected.borrow();
            let Some(selection) = selection.as_ref() else {
                return false;
            };
            list.get(position as usize)
                .is_some_and(|v| v.id() == selection.id())
        }

        fn selection_in_range(&self, position: u32, n_items: u32) -> gtk::Bitset {
            let result = gtk::Bitset::new_range(position, n_items);
            let list = self.items.borrow();
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
