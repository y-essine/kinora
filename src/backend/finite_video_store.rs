use gdk::gio;
use gdk::prelude::{Cast, ListModelExt};
use gdk::subclass::prelude::ObjectSubclassIsExt;
use glib::object::ObjectExt;
use glib::{Object, clone};

use super::Video;

// TODO: Section model?
gtk::glib::wrapper! {
    pub struct FiniteVideoStore(ObjectSubclass<imp::FiniteVideoStore>)
        @implements gio::ListModel, gtk::SelectionModel;
}

impl Default for FiniteVideoStore {
    fn default() -> Self {
        Object::builder().build()
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub enum VideoSort {
    UploadedAscending,
    #[default]
    UploadedDescending,
    WatchedAscending,
    WatchedDescending,
}

impl VideoSort {
    fn compare(&self, a: &Video, b: &Video) -> std::cmp::Ordering {
        let primary_comparison = match self {
            VideoSort::UploadedAscending => a.uploaded().cmp(&b.uploaded()).reverse(),
            VideoSort::UploadedDescending => a.uploaded().cmp(&b.uploaded()),
            VideoSort::WatchedAscending => a.watched_at().cmp(&b.watched_at()).reverse(),
            VideoSort::WatchedDescending => a.watched_at().cmp(&b.watched_at()),
        };

        primary_comparison
            .then_with(|| a.id().id.cmp(&b.id().id))
            .then_with(|| a.platform().cmp(&b.platform()))
    }
}

impl FiniteVideoStore {
    pub fn with_sort(sort: VideoSort) -> Self {
        let s = Self::default();
        s.set_sort(sort);
        s
    }

    /// Adds a video to the list if it is not yet contained in it.
    pub fn add(&self, video: Video) {
        if !self.toggle(video.clone()) {
            self.toggle(video);
        }
    }

    /// Removes a video to the list if it contained in it.
    pub fn remove(&self, video: Video) {
        if self.toggle(video.clone()) {
            self.toggle(video);
        }
    }

    /// Toggle a video in the list.
    ///
    /// Returns `true` if the video was added to the list, and `false` if it was removed.
    pub fn toggle(&self, video: Video) -> bool {
        let mut videos = self.imp().videos.borrow_mut();
        let sort = self.imp().sort.get();
        let index = videos.binary_search_by(|v| sort.compare(&video, v));

        match index {
            Ok(index) => {
                // Remove any installed signal handler.
                if let Some(handler) = self.imp().signal_handlers.borrow_mut().remove(&video.id()) {
                    video.disconnect(handler);
                } else {
                    log::warn!(
                        "Removing a video from the watch later list, which does not have a handler installed. This should not happen."
                    );
                }

                videos.remove(index);
                drop(videos);
                self.items_changed(index as u32, 1, 0);
                self.notify_is_empty();
                false
            }
            Err(index) => {
                // As the uploaded time can be inaccurate in the case of YouTube (they only say e.g. "1 day ago"), the uploaded date can vary over time in the watch later list.
                // Add a handler to reinsert the element in the watch later list when that happens.
                let handler = video.connect_uploaded_unix_notify(clone!(
                    #[weak(rename_to=s)]
                    self,
                    move |video| {
                        log::trace!("Video uploaded changed; reinserting into watch later list");
                        // Note: One cannot remove the video again by calling "toggle", as it would binary search with the new timestamp and not the old one.
                        // One has to do a full scan of all videos to find where the video was before.
                        s.remove_video_full_search(video);
                        s.toggle(video.clone());
                    }
                ));
                self.imp()
                    .signal_handlers
                    .borrow_mut()
                    .insert(video.id(), handler);

                videos.insert(index, video);
                drop(videos);
                self.items_changed(index as u32, 0, 1);
                self.notify_is_empty();
                true
            }
        }
    }

    fn remove_video_full_search(&self, video: &Video) {
        let mut videos = self.imp().videos.borrow_mut();
        if let Some(index) = videos.iter().position(|v| v == video) {
            videos.remove(index);
            drop(videos);
            self.items_changed(index as u32, 1, 0);
            // Remove any installed signal handler.
            if let Some(handler) = self.imp().signal_handlers.borrow_mut().remove(&video.id()) {
                video.disconnect(handler);
            } else {
                log::warn!(
                    "Removing a video from the watch later list, which does not have a handler installed. This should not happen."
                );
            }
            self.notify_is_empty();
        }
    }

    pub fn set_sort(&self, sort: VideoSort) {
        self.imp().sort.set(sort);

        // Re-sort the videos using the same comparator as binary search
        let mut videos = self.imp().videos.borrow_mut();
        videos.sort_by(|a, b| sort.compare(a, b));
        drop(videos);

        // Notify that all items changed
        let n = self.imp().videos.borrow().len() as u32;
        self.items_changed(0, n, n);
    }

    pub fn clear(&self) {
        let mut videos = self.imp().videos.borrow_mut();
        let removed = videos.len();
        videos.clear();
        drop(videos);
        self.items_changed(0, removed as u32, 0);
    }

    fn items_changed(&self, position: u32, removed: u32, added: u32) {
        self.upcast_ref::<gio::ListModel>()
            .items_changed(position, removed, added);
    }
}

mod imp {
    use gdk::gio;
    use gdk::prelude::Cast;
    use glib::SignalHandlerId;
    use glib::subclass::types::ObjectSubclassExt;
    use glib::types::StaticType;
    use gtk::glib;
    use gtk::prelude::SelectionModelExt;
    use gtk::subclass::selection_model::SelectionModelImpl;
    use pcore::VideoId;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    use gdk::subclass::prelude::{DerivedObjectProperties, ListModelImpl};
    use gdk::{
        glib::Properties,
        prelude::ObjectExt,
        subclass::prelude::{ObjectImpl, ObjectSubclass},
    };

    use crate::backend::Video;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::FiniteVideoStore)]
    pub struct FiniteVideoStore {
        #[property(name = "is-empty", type = bool, get = |s: &Self| s.videos.borrow().is_empty())]
        pub(super) videos: RefCell<Vec<Video>>,

        pub(super) signal_handlers: RefCell<HashMap<VideoId, SignalHandlerId>>,

        #[property(get, set = Self::set_selected, nullable)]
        selected: RefCell<Option<Video>>,

        pub(super) sort: Cell<super::VideoSort>,
    }

    impl FiniteVideoStore {
        fn set_selected(&self, selected: Option<Video>) {
            self.selected.replace(selected);
            // Don't know where exactly it changed; emit that it changed everywhere.
            let num_videos = self.videos.borrow().len() as u32;
            if num_videos > 0 {
                self.obj().selection_changed(0, num_videos);
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FiniteVideoStore {
        const NAME: &'static str = "TFFiniteVideoStore";
        type Type = super::FiniteVideoStore;
        type Interfaces = (gio::ListModel, gtk::SelectionModel);
    }

    #[glib::derived_properties]
    impl ObjectImpl for FiniteVideoStore {}

    impl ListModelImpl for FiniteVideoStore {
        fn item_type(&self) -> glib::Type {
            Video::static_type()
        }

        fn n_items(&self) -> u32 {
            self.videos.borrow().len().try_into().unwrap_or_default()
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let list = self.videos.borrow();

            list.get(position as usize)
                .map(|o| o.clone().upcast::<glib::Object>())
        }
    }

    impl SelectionModelImpl for FiniteVideoStore {
        fn is_selected(&self, position: u32) -> bool {
            let list = self.videos.borrow();
            let selection = self.selected.borrow();
            let Some(selection) = selection.as_ref() else {
                return false;
            };
            list.get(position as usize)
                .is_some_and(|v| v.id() == selection.id())
        }

        fn selection_in_range(&self, position: u32, n_items: u32) -> gtk::Bitset {
            let result = gtk::Bitset::new_range(position, n_items);
            let list = self.videos.borrow();
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
