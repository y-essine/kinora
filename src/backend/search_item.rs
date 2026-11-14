use glib::Object;
use pcore::{ChannelId, VideoId};

use super::{Channel, Video};

gtk::glib::wrapper! {
    pub struct SearchItem(ObjectSubclass<imp::SearchItem>);
}

impl SearchItem {
    pub fn for_video(video: &Video) -> Self {
        Object::builder().property("video", video).build()
    }

    pub fn for_channel(channel: &Channel) -> Self {
        Object::builder().property("channel", channel).build()
    }

    // More readable as we have more than three cases.
    #[allow(clippy::manual_map)]
    pub fn id(&self) -> Option<SearchItemId> {
        if let Some(video) = self.video() {
            Some(SearchItemId::Video(video.id()))
        } else if let Some(channel) = self.channel() {
            Some(SearchItemId::Channel(channel.id()))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SearchItemId {
    Video(VideoId),
    Channel(ChannelId), // TODO: Playlist
}

mod imp {
    use glib::Properties;
    use gtk::glib;
    use std::cell::RefCell;

    use gdk::prelude::ObjectExt;
    use gdk::subclass::prelude::{DerivedObjectProperties, ObjectImpl, ObjectSubclass};

    use crate::backend::{Channel, Video};

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::SearchItem)]
    pub struct SearchItem {
        #[property(get, set, nullable, construct_only)]
        channel: RefCell<Option<Channel>>,
        #[property(get, set, nullable, construct_only)]
        video: RefCell<Option<Video>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SearchItem {
        const NAME: &'static str = "TFSearchItem";
        type Type = super::SearchItem;
    }

    #[glib::derived_properties]
    impl ObjectImpl for SearchItem {}
}
