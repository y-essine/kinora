use gtk::glib::Object;

gtk::glib::wrapper! {
    pub struct SearchListItem(ObjectSubclass<imp::SearchListItem>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

impl Default for SearchListItem {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchListItem {
    pub fn new() -> Self {
        let s: Self = Object::builder::<Self>().build();
        s
    }
}

pub mod imp {
    use std::cell::Cell;
    use std::cell::RefCell;

    use glib::Properties;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::CompositeTemplate;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use std::sync::LazyLock;

    use crate::backend::Client;
    use crate::backend::PlayType;
    use crate::backend::SearchItem;
    use crate::backend::Video;
    use crate::gui::utility::Utility;

    #[derive(CompositeTemplate, Default, Properties)]
    #[template(resource = "/ui/search_list_item.ui")]
    #[properties(wrapper_type = super::SearchListItem)]
    pub struct SearchListItem {
        #[property(get, set)]
        item: RefCell<Option<SearchItem>>,

        #[property(get, set)]
        compact: Cell<bool>,
        #[property(get, set)]
        client: RefCell<Client>,
    }

    #[gtk::template_callbacks]
    impl SearchListItem {
        #[template_callback]
        fn play_video(&self, video: Video, play_type: PlayType) {
            self.obj()
                .emit_by_name::<()>("play-video", &[&video, &play_type]);
        }
        #[template_callback]
        fn copy_video_url(&self, video: Video) {
            self.obj().emit_by_name::<()>("copy-video-url", &[&video]);
        }
        #[template_callback]
        fn download(&self, video: Video) {
            self.obj().emit_by_name::<()>("download", &[&video]);
        }
        #[template_callback]
        fn remove_downloaded(&self, video: Video) {
            self.obj()
                .emit_by_name::<()>("remove-downloaded", &[&video]);
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SearchListItem {
        const NAME: &'static str = "TFSearchListItem";
        type Type = super::SearchListItem;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
            Utility::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for SearchListItem {
        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("play-video")
                        .param_types([Video::static_type(), PlayType::static_type()])
                        .build(),
                    Signal::builder("copy-video-url")
                        .param_types([Video::static_type()])
                        .build(),
                    Signal::builder("download")
                        .param_types([Video::static_type()])
                        .build(),
                    Signal::builder("remove-downloaded")
                        .param_types([Video::static_type()])
                        .build(),
                ]
            });
            SIGNALS.as_ref()
        }
    }

    impl WidgetImpl for SearchListItem {}
    impl BoxImpl for SearchListItem {}
}
