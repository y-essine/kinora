use gdk::glib::Object;

gtk::glib::wrapper! {
    pub struct Filter(ObjectSubclass<imp::Filter>)
    @extends gtk::Filter;
}

impl Filter {
    pub fn new<S: AsRef<str>>(platform: S, title: S, channel: S) -> Self {
        Object::builder::<Self>()
            .property("platform", platform.as_ref())
            .property("title", title.as_ref())
            .property("channel", channel.as_ref())
            .build()
    }
}

mod imp {
    use glib::Properties;
    use glib::object::Cast;
    use glib::subclass::types::ObjectSubclassExt;
    use gtk::glib;
    use gtk::subclass::filter::{FilterImpl, FilterImplExt};
    use std::cell::RefCell;

    use gdk::prelude::ObjectExt;
    use gdk::subclass::prelude::{DerivedObjectProperties, ObjectImpl, ObjectSubclass};

    use crate::backend::Video;

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::Filter)]
    pub struct Filter {
        #[property(get, set, construct_only)]
        platform: RefCell<String>,
        #[property(get, set, construct_only)]
        title: RefCell<String>,
        #[property(get, set, construct_only)]
        channel: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Filter {
        const NAME: &'static str = "TFFilter";
        type Type = super::Filter;
        type ParentType = gtk::Filter;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Filter {}

    impl FilterImpl for Filter {
        fn strictness(&self) -> gtk::FilterMatch {
            self.parent_strictness()
        }

        fn match_(&self, item: &glib::Object) -> bool {
            let obj = self.obj();
            let Some(video) = item.dynamic_cast_ref::<Video>() else {
                return false;
            };

            let platform_matches = video.id().platform.contains(&obj.platform());
            let channel_matches = video.uploader().name().contains(&obj.channel());
            let title_matches = video
                .title()
                .to_lowercase()
                .contains(&obj.title().to_lowercase());

            !(platform_matches && channel_matches && title_matches)
        }
    }
}
