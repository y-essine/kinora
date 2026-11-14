use std::cmp::Ordering;

use gdk::gio;
use gdk::prelude::{Cast, ListModelExt};
use gdk::subclass::prelude::ObjectSubclassIsExt;
use glib::object::ObjectExt;
use glib::{Object, clone};

use super::Channel;

// TODO: Section model?
gtk::glib::wrapper! {
    pub struct SubscriptionsStore(ObjectSubclass<imp::SubscriptionsStore>)
        @implements gio::ListModel, gtk::SelectionModel;
}

impl Default for SubscriptionsStore {
    fn default() -> Self {
        Object::builder().build()
    }
}

impl SubscriptionsStore {
    /// Toggle a channel in the list.
    ///
    /// Returns `true` if the channel was added to the list, and `false` if it was removed.
    pub fn toggle(&self, channel: Channel) -> bool {
        let mut channels = self.imp().channels.borrow_mut();
        let mut handlers = self.imp().signal_handlers.borrow_mut();

        if let Some(index) = channels.iter().position(|c| c.id() == channel.id()) {
            channels.remove(index);
            if let Some(old_handler) = handlers.remove(&channel.id()) {
                channel.disconnect(old_handler);
            }
            drop(channels);

            self.items_changed(index as u32, 1, 0);
            self.notify_is_empty();
            return false;
        }

        drop(channels);
        drop(handlers);
        self.add_if_not_exists(channel)
    }

    pub fn add_if_not_exists(&self, channel: Channel) -> bool {
        let mut channels = self.imp().channels.borrow_mut();
        let mut handlers = self.imp().signal_handlers.borrow_mut();

        if let Err(index) = channels.binary_search_by(|v| Self::compare_channels(v, &channel)) {
            let handler = channel.connect_name_notify(clone!(
                #[weak(rename_to = s)]
                self,
                move |channel| {
                    // Reorder by removing and adding the channel again.
                    s.toggle(channel.clone());
                    s.toggle(channel.clone());
                }
            ));
            handlers.insert(channel.id(), handler);

            channels.insert(index, channel);
            drop(channels);
            self.items_changed(index as u32, 0, 1);
            self.notify_is_empty();
            return true;
        }
        false
    }

    fn compare_channels(channel1: &Channel, channel2: &Channel) -> Ordering {
        channel1
            .name()
            .to_lowercase()
            .cmp(&channel2.name().to_lowercase())
            .then_with(|| channel1.id().id.cmp(&channel2.id().id))
    }

    pub fn channels(&self) -> Vec<Channel> {
        self.imp().channels.borrow().clone()
    }

    pub fn get(&self, idx: usize) -> Option<Channel> {
        self.imp().channels.borrow().get(idx).cloned()
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
    use pcore::ChannelId;
    use std::cell::RefCell;
    use std::collections::HashMap;

    use gdk::subclass::prelude::{DerivedObjectProperties, ListModelImpl};
    use gdk::{
        glib::Properties,
        prelude::ObjectExt,
        subclass::prelude::{ObjectImpl, ObjectSubclass},
    };

    use crate::backend::Channel;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::SubscriptionsStore)]
    pub struct SubscriptionsStore {
        #[property(name = "is-empty", type = bool, get = |s: &Self| s.channels.borrow().is_empty())]
        pub(super) channels: RefCell<Vec<Channel>>,

        pub(super) signal_handlers: RefCell<HashMap<ChannelId, SignalHandlerId>>,

        #[property(get, set = Self::set_selected, nullable)]
        selected: RefCell<Option<Channel>>,
    }

    impl SubscriptionsStore {
        fn set_selected(&self, selected: Option<Channel>) {
            self.selected.replace(selected);
            // Don't know where exactly it changed; emit that it changed everywhere.
            let num_videos = self.channels.borrow().len() as u32;
            if num_videos > 0 {
                self.obj().selection_changed(0, num_videos);
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SubscriptionsStore {
        const NAME: &'static str = "TFSubscriptionsStore";
        type Type = super::SubscriptionsStore;
        type Interfaces = (gio::ListModel, gtk::SelectionModel);
    }

    #[glib::derived_properties]
    impl ObjectImpl for SubscriptionsStore {}

    impl ListModelImpl for SubscriptionsStore {
        fn item_type(&self) -> glib::Type {
            Channel::static_type()
        }

        fn n_items(&self) -> u32 {
            self.channels.borrow().len().try_into().unwrap_or_default()
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let list = self.channels.borrow();

            list.get(position as usize)
                .map(|o| o.clone().upcast::<glib::Object>())
        }
    }

    impl SelectionModelImpl for SubscriptionsStore {
        fn is_selected(&self, position: u32) -> bool {
            let list = self.channels.borrow();
            let selection = self.selected.borrow();
            let Some(selection) = selection.as_ref() else {
                return false;
            };
            list.get(position as usize)
                .is_some_and(|v| v.id() == selection.id())
        }

        fn selection_in_range(&self, position: u32, n_items: u32) -> gtk::Bitset {
            let result = gtk::Bitset::new_range(position, n_items);
            let list = self.channels.borrow();
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
