use std::cell::RefCell;

use chrono::{DateTime, TimeDelta, Utc};
use gdk::glib::Object;
use gdk::subclass::prelude::ObjectSubclassIsExt;
use pcore::VideoId;
use url::Url;

use super::Channel;

#[derive(glib::Enum, Clone, Copy, Debug, PartialEq, Eq)]
#[enum_type(name = "TFPlayType")]
pub enum PlayType {
    Default,
    Inverted,
}

gtk::glib::wrapper! {
    pub struct Video(ObjectSubclass<imp::Video>);
}

impl Video {
    pub fn new(video: pcore::Video, uploader: &Channel) -> Self {
        let s: Self = Object::builder::<Self>()
            .property("uploader", uploader)
            .build();
        s.imp().video.swap(&RefCell::new(video));
        s
    }

    pub fn thumbnail_url(&self) -> Option<Url> {
        self.imp().video.borrow().thumbnail.clone()
    }

    pub fn playable_url(&self) -> Url {
        self.imp().video.borrow().playable_url.clone()
    }

    pub fn id(&self) -> VideoId {
        self.imp().video.borrow().id.clone()
    }

    pub fn update(&self, video: pcore::Video) {
        let old = self.imp().video.replace_with(|v| v.clone().merge(video));
        let new = self.video();

        if old.title != new.title {
            self.notify_title();
        }
        if old.uploaded != new.uploaded {
            self.notify_uploaded_unix();
            self.notify_uploaded_label();
        }
        if old.duration != new.duration {
            self.notify_duration_label();
        }
        if old.description != new.description {
            self.notify_description();
        }
        if old.views != new.views {
            self.notify_views();
            self.notify_has_views();
        }
        if old.likes != new.likes {
            self.notify_likes();
            self.notify_has_likes();
        }
        if old.dislikes != new.dislikes {
            self.notify_dislikes();
            self.notify_has_dislikes();
        }
        if old.is_live != new.is_live {
            self.notify_is_live();
        }
        if old.requires_payment != new.requires_payment {
            self.notify_requires_payment();
        }
        if old.id.platform != new.id.platform {
            self.notify_platform();
        }
    }

    pub fn uploaded(&self) -> DateTime<Utc> {
        self.imp().video.borrow().uploaded
    }

    pub fn watched_at(&self) -> Option<DateTime<Utc>> {
        *self.imp().watched_at.borrow()
    }

    pub fn set_watched_at(&self, value: Option<DateTime<Utc>>) {
        *self.imp().watched_at.borrow_mut() = value;
        self.notify_watched();
    }

    pub fn duration(&self) -> Option<TimeDelta> {
        self.imp().video.borrow().duration
    }

    pub fn video(&self) -> pcore::Video {
        self.imp().video.borrow().clone()
    }
}

mod imp {
    use chrono::{DateTime, TimeZone, Utc};
    use gdk::gdk_pixbuf::Pixbuf;
    use glib::Properties;
    use glib::subclass::types::ObjectSubclassExt;
    use gtk::glib;
    use std::cell::{Cell, RefCell};
    use std::marker::PhantomData;
    use url::Url;

    use gdk::prelude::ObjectExt;
    use gdk::subclass::prelude::{DerivedObjectProperties, ObjectImpl, ObjectSubclass};

    use crate::backend::Channel;
    use crate::gui::utility::Utility;

    #[derive(Properties)]
    #[properties(wrapper_type = super::Video)]
    pub struct Video {
        #[property(name = "title", type = String, get = |d: &Self| d.video.borrow().title.lines().next().unwrap().to_owned())]
        #[property(name = "uploaded-unix", type = i64, get = |d: &Self| d.video.borrow().uploaded.timestamp())]
        #[property(name = "uploaded-label", type = String, get = |d: &Self| Utility::format_date_human(&d.video.borrow().uploaded))]
        #[property(name = "duration-label", type = String, get = |d: &Self| d.video.borrow().duration.map(Utility::format_duration_human).unwrap_or_default())]
        #[property(name = "description", type = Option<String>, get = |d: &Self| d.video.borrow().description.clone())]
        #[property(name = "views", type = u64, get = |d: &Self| d.video.borrow().views.unwrap_or_default())]
        #[property(name = "has-views", type = bool, get = |d: &Self| d.video.borrow().views.is_some())]
        #[property(name = "likes", type = u64, get = |d: &Self| d.video.borrow().likes.unwrap_or_default())]
        #[property(name = "has-likes", type = bool, get = |d: &Self| d.video.borrow().likes.is_some())]
        #[property(name = "dislikes", type = u64, get = |d: &Self| d.video.borrow().dislikes.unwrap_or_default())]
        #[property(name = "has-dislikes", type = bool, get = |d: &Self| d.video.borrow().dislikes.is_some())]
        #[property(name = "is-live", type = bool, get = |d: &Self| d.video.borrow().is_live.unwrap_or_default())]
        #[property(name = "requires-payment", type = bool, get = |d: &Self| d.video.borrow().requires_payment.unwrap_or_default())]
        #[property(name = "platform", type = String, get = |d: &Self| d.video.borrow().id.platform.to_string())]
        pub(super) video: RefCell<pcore::Video>,

        #[property(get, set, nullable)]
        thumbnail: RefCell<Option<Pixbuf>>,

        #[property(get, set)]
        watch_later: Cell<bool>,
        #[property(get, set)]
        download_progress: Cell<f64>,
        #[property(get, set)]
        downloading: Cell<bool>,
        #[property(get, set = Self::set_downloaded_path, nullable)]
        #[property(name = "downloaded", type = bool, get = |d: &Self| d.downloaded_path.borrow().is_some())]
        downloaded_path: RefCell<Option<String>>,

        // Kinora: settable, so a video first seen without its channel
        // ("Up next") can take the real one once it is known.
        #[property(get, set)]
        uploader: RefCell<Channel>,

        pub(super) watched_at: RefCell<Option<DateTime<Utc>>>,
        #[property(get = |d: &Self| d.watched_at.borrow().is_some())]
        watched: PhantomData<bool>,
    }

    impl Video {
        fn set_downloaded_path(&self, p: Option<String>) {
            *self.downloaded_path.borrow_mut() = p;
            self.obj().notify("downloaded");
        }
    }

    // Does not matter anyways, will get overwritten by the constructor.
    impl Default for Video {
        fn default() -> Self {
            Self {
                video: RefCell::new(pcore::Video {
                    id: pcore::VideoId {
                        id: Default::default(),
                        platform: std::borrow::Cow::Borrowed(""),
                    },
                    playable_url: Url::parse("https://example.com").unwrap(),
                    uploader: pcore::Channel {
                        id: pcore::ChannelId {
                            id: Default::default(),
                            platform: std::borrow::Cow::Borrowed(""),
                        },
                        name: Default::default(),
                        avatar: Default::default(),
                        banner: Default::default(),
                        description: Default::default(),
                        subscribers: Default::default(),
                    },
                    title: Default::default(),
                    uploaded: Utc.timestamp_opt(0, 0).unwrap(),
                    duration: Default::default(),
                    thumbnail: None,
                    description: Default::default(),
                    views: Default::default(),
                    likes: Default::default(),
                    dislikes: Default::default(),
                    is_live: Default::default(),
                    requires_payment: Default::default(),
                }),
                thumbnail: Default::default(),
                watch_later: Default::default(),
                watched: Default::default(),
                download_progress: Default::default(),
                downloading: Default::default(),
                downloaded_path: Default::default(),
                uploader: RefCell::new(Channel::new(pcore::Channel {
                    id: pcore::ChannelId {
                        id: Default::default(),
                        platform: std::borrow::Cow::Borrowed(""),
                    },
                    name: Default::default(),
                    avatar: Default::default(),
                    banner: Default::default(),
                    description: Default::default(),
                    subscribers: Default::default(),
                })),
                watched_at: Default::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Video {
        const NAME: &'static str = "TFVideo";
        type Type = super::Video;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Video {}
}
