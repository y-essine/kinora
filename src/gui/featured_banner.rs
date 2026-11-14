//! Kinora: the featured banner on top of Home: one video, shown large.
//!
//! A reusable building block (see `docs/design.md`): set `video` and `kicker`
//! and listen to `play-video`.

gtk::glib::wrapper! {
    pub struct FeaturedBanner(ObjectSubclass<imp::FeaturedBanner>)
        @extends adw::BreakpointBin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl FeaturedBanner {
    /// The picture, for the player to grow out of (see `player_sheet.rs`).
    pub fn picture_widget(&self) -> gtk::Picture {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        self.imp().picture.get()
    }
}

pub mod imp {
    use adw::subclass::breakpoint_bin::BreakpointBinImpl;
    use glib::Properties;
    use glib::clone;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::CompositeTemplate;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use std::cell::RefCell;
    use std::sync::LazyLock;

    use crate::backend::Client;
    use crate::backend::PlayType;
    use crate::backend::Video;
    use crate::gui::utility::Utility;

    #[derive(CompositeTemplate, Properties, Default)]
    #[template(resource = "/ui/featured_banner.ui")]
    #[properties(wrapper_type = super::FeaturedBanner)]
    pub struct FeaturedBanner {
        #[template_child]
        pub(super) picture: TemplateChild<gtk::Picture>,

        #[property(get, set = Self::set_video, nullable)]
        video: RefCell<Option<Video>>,
        /// The small line above the title, e.g. "Featured".
        #[property(get, set)]
        kicker: RefCell<String>,
        #[property(get, set)]
        client: RefCell<Client>,
    }

    #[gtk::template_callbacks]
    impl FeaturedBanner {
        fn set_video(&self, video: Option<Video>) {
            if *self.video.borrow() == video {
                return;
            }
            self.video.replace(video.clone());
            // The banner is not a list item: fetch its picture itself.
            if let Some(video) = video
                && video.thumbnail().is_none()
            {
                let client = self.obj().client();
                crate::gspawn!(async move { client.populate_thumbnail(&video).await });
            }
        }

        #[template_callback]
        fn play(&self) {
            if let Some(video) = self.obj().video() {
                self.obj()
                    .emit_by_name::<()>("play-video", &[&video, &PlayType::Default]);
            }
        }

        #[template_callback]
        fn banner_clicked(&self, _n: i32, _x: f64, _y: f64) {
            self.play();
        }

        #[template_callback]
        fn watch_later(&self) {
            if let Some(video) = self.obj().video() {
                self.obj().client().toggle_watch_later(video);
            }
        }

        /// "Channel · 3 weeks ago · 12:04", leaving out what is not known.
        #[template_callback(function)]
        fn banner_meta(
            channel: Option<&str>,
            date: Option<&str>,
            duration: Option<&str>,
        ) -> String {
            [channel, date, duration]
                .into_iter()
                .flatten()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("  ·  ")
        }

        fn setup(&self) {
            // Fade the picture in when it arrives (the CSS class carries the transition).
            self.picture.connect_paintable_notify(clone!(
                #[weak(rename_to = s)]
                self,
                move |picture| {
                    if picture.paintable().is_some() {
                        s.picture.add_css_class("loaded");
                    } else {
                        s.picture.remove_css_class("loaded");
                    }
                }
            ));
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FeaturedBanner {
        const NAME: &'static str = "TFFeaturedBanner";
        type Type = super::FeaturedBanner;
        type ParentType = adw::BreakpointBin;

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
    impl ObjectImpl for FeaturedBanner {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("play-video")
                        .param_types([Video::static_type(), PlayType::static_type()])
                        .build(),
                ]
            });
            SIGNALS.as_ref()
        }
    }

    impl WidgetImpl for FeaturedBanner {}
    impl BreakpointBinImpl for FeaturedBanner {}
}
