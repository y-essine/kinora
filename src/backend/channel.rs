use std::cell::RefCell;

use gdk::glib::Object;
use gdk::subclass::prelude::ObjectSubclassIsExt;
use pcore::ChannelId;
use url::Url;

gtk::glib::wrapper! {
    pub struct Channel(ObjectSubclass<imp::Channel>);
}

impl Channel {
    pub fn new(channel: pcore::Channel) -> Self {
        let s: Self = Object::builder::<Self>().build();
        s.imp().channel.swap(&RefCell::new(channel));
        s
    }

    pub fn avatar_url(&self) -> Option<Url> {
        self.imp().channel.borrow().avatar.clone()
    }

    pub fn id(&self) -> ChannelId {
        self.imp().channel.borrow().id.clone()
    }

    pub fn update(&self, channel: pcore::Channel) {
        let old = self
            .imp()
            .channel
            .replace_with(|c| c.clone().merge(channel));
        let new = self.channel();

        if old.name != new.name {
            self.notify_name();
        }
        if old.description != new.description {
            self.notify_description();
        }
        if old.subscribers != new.subscribers {
            self.notify_subscribers();
            self.notify_has_subscribers();
        }
        if old.id.platform != new.id.platform {
            self.notify_platform()
        }
    }

    pub fn channel(&self) -> pcore::Channel {
        self.imp().channel.borrow().clone()
    }
}

mod imp {
    use gdk::gdk_pixbuf::Pixbuf;
    use glib::Properties;
    use gtk::glib;
    use std::cell::{Cell, RefCell};

    use gdk::prelude::ObjectExt;
    use gdk::subclass::prelude::{DerivedObjectProperties, ObjectImpl, ObjectSubclass};

    #[derive(Properties)]
    #[properties(wrapper_type = super::Channel)]
    pub struct Channel {
        #[property(name = "name", member = name, type = String, get = Self::get_name)]
        #[property(name = "description", type = Option<String>, get = |d: &Self| d.channel.borrow().description.clone())]
        #[property(name = "subscribers", type = u64, get = |d: &Self| d.channel.borrow().subscribers.unwrap_or_default())]
        #[property(name = "has-subscribers", type = bool, get = |d: &Self| d.channel.borrow().subscribers.is_some())]
        #[property(name = "platform", type = String, get = |d: &Self| d.channel.borrow().id.platform.to_string())]
        pub(super) channel: RefCell<pcore::Channel>,

        #[property(get, set)]
        avatar: RefCell<Option<Pixbuf>>,

        #[property(get, set)]
        subscribed: Cell<bool>,
    }

    impl Channel {
        /// Get the name of the channel.
        ///
        /// If the name is not yet known (when importing from the old store and the has no video in the feed yet), return a fallback.
        fn get_name(&self) -> String {
            let name = &self.channel.borrow().name;
            if name.is_empty() {
                gettextrs::gettext("Unknown Channel")
            } else {
                name.to_owned()
            }
        }
    }

    // Does not matter anyways, will get overwritten by the constructor.
    impl Default for Channel {
        fn default() -> Self {
            Self {
                channel: RefCell::new(pcore::Channel {
                    id: pcore::ChannelId {
                        id: Default::default(),
                        platform: std::borrow::Cow::Borrowed(""),
                    },
                    name: Default::default(),
                    avatar: Default::default(),
                    banner: Default::default(),
                    description: Default::default(),
                    subscribers: Default::default(),
                }),
                avatar: Default::default(),
                subscribed: Default::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Channel {
        const NAME: &'static str = "TFChannel";
        type Type = super::Channel;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Channel {}
}
