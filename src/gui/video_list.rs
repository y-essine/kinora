use std::time::Duration;

use gdk::{
    gio::{SimpleAction, SimpleActionGroup},
    glib,
    glib::clone,
    prelude::ActionMapExt,
    subclass::prelude::ObjectSubclassIsExt,
};
use glib::object::Cast;
use gtk::{
    Adjustment,
    prelude::{AdjustmentExt, WidgetExt},
};

use crate::gspawn;

use super::window::Window;

const LOAD_COUNT: usize = 10;

gtk::glib::wrapper! {
    pub struct VideoList(ObjectSubclass<imp::VideoList>)
        @extends adw::BreakpointBin, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

impl VideoList {
    fn add_actions(&self) {
        let action_more = SimpleAction::new("more", None);
        action_more.connect_activate(clone!(
            #[weak(rename_to = s)]
            self,
            move |_, _| {
                if let Some(model) = s.model() {
                    model.load_more(LOAD_COUNT);
                }
            }
        ));
        let action_reload = SimpleAction::new("reload", None);
        action_reload.connect_activate(clone!(
            #[weak(rename_to = s)]
            self,
            move |_, _| {
                s.reload();
            }
        ));

        let actions = SimpleActionGroup::new();
        self.insert_action_group("feed", Some(&actions));
        actions.add_action(&action_more);
        actions.add_action(&action_reload);
    }

    pub fn reload(&self) {
        let client = self.client();
        client.clear_title_cache();
        let subscriptions = client
            .subscriptions()
            .into_iter()
            .map(|c| c.id())
            .collect::<Vec<_>>();
        gspawn!(clone!(
            #[weak(rename_to = s)]
            self,
            #[weak]
            client,
            #[strong]
            subscriptions,
            async move {
                let stream = client.videos_for_channels(&subscriptions, true).await;
                stream.load_more(LOAD_COUNT);
                s.set_model(stream);
            }
        ));
    }

    fn setup_autoload(&self) {
        let adj = self.imp().scrolled_window.vadjustment();
        adj.connect_changed(clone!(
            #[weak(rename_to = s)]
            self,
            move |adj| {
                s.load_if_screen_not_filled(adj);
            }
        ));
    }

    fn load_if_screen_not_filled(&self, adj: &Adjustment) {
        let ctx = glib::MainContext::default();
        ctx.spawn_local(clone!(
            #[weak(rename_to = s)]
            self,
            #[weak]
            adj,
            async move {
                // Add timeout, otherwise there are some critical errors (not sure why).
                glib::timeout_future(Duration::from_millis(100)).await;
                if s.model().is_some_and(|m| !m.finished()) && adj.upper() <= adj.page_size() {
                    // The screen is not yet filled.
                    let _ = s.activate_action("feed.more", None);
                }
            }
        ));
    }

    fn window(&self) -> Window {
        self.root().unwrap().dynamic_cast().unwrap()
    }
}

pub mod imp {
    use adw::subclass::breakpoint_bin::BreakpointBinImpl;
    use glib::BoxedAnyObject;
    use glib::Object;
    use glib::Properties;
    use glib::clone;
    use glib::closure;
    use glib::closure_local;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::PositionType;
    use gtk::SignalListItemFactory;
    use gtk::Widget;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::sync::LazyLock;

    use gtk::CompositeTemplate;

    use crate::backend::PlayType;
    use crate::backend::Video;
    use crate::backend::{Client, VideoFetcherStream};
    use crate::gui::utility::Utility;
    use crate::gui::video_item::VideoItem;

    #[derive(CompositeTemplate, Properties, Default)]
    #[template(resource = "/ui/video_list.ui")]
    #[properties(wrapper_type = super::VideoList)]
    pub struct VideoList {
        #[template_child]
        pub(super) feed_list: TemplateChild<gtk::GridView>,
        #[template_child]
        pub(super) scrolled_window: TemplateChild<gtk::ScrolledWindow>,

        #[property(get, set = Self::set_model)]
        model: RefCell<Option<VideoFetcherStream>>,
        #[property(get, set, nullable)]
        selected: RefCell<Option<Video>>,
        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,
        #[property(get, set)]
        filter_out_similar_enabled: Cell<bool>,

        #[property(get, set)]
        clamped_to: Cell<u32>,

        #[property(get, set)]
        client: RefCell<Client>,
    }

    impl VideoList {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);

            if compact {
                self.feed_list.set_max_columns(1);
            } else {
                self.feed_list.set_max_columns(6);
            }
        }

        fn set_model(&self, model: VideoFetcherStream) {
            let obj = self.obj();
            self.feed_list.get().set_model(Some(&model));
            obj.property_expression("selected")
                .bind(&model, "selected", Widget::NONE);

            model.connect_local(
                "error",
                false,
                clone!(
                    #[weak]
                    obj,
                    #[upgrade_or_default]
                    move |err| {
                        let err = err[1]
                            .get::<BoxedAnyObject>()
                            .expect("Error to be BoxedAnyObject");
                        let err: &(String, RefCell<Option<pcore::Error>>) = &err.borrow();
                        obj.window().display_error(
                            err.0.clone(),
                            Box::new(err.1.take().expect("Error to be present")),
                        );
                        None
                    }
                ),
            );

            self.model.borrow_mut().replace(model);
        }

        pub(super) fn setup(&self) {
            let obj = self.obj();

            let factory = SignalListItemFactory::new();
            factory.connect_setup(clone!(
                #[weak]
                obj,
                move |_, list_item| {
                    let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
                    let video_item = VideoItem::new();
                    video_item.set_filter_out_similar_enabled(true);
                    list_item.set_child(Some(&video_item));

                    list_item
                        .property_expression("item")
                        .bind(&video_item, "video", Widget::NONE);
                    obj.property_expression("client")
                        .bind(&video_item, "client", Widget::NONE);
                    obj.property_expression("compact")
                        .bind(&video_item, "compact", Widget::NONE);
                    obj.property_expression("filter-out-similar-enabled").bind(
                        &video_item,
                        "filter-out-similar-enabled",
                        Widget::NONE,
                    );
                    video_item
                        .property_expression("video")
                        .chain_closure::<Option<String>>(closure!(
                            |_: Option<Object>, video: Option<Video>| {
                                video.map(Utility::accessible_description_for_video)
                            }
                        ))
                        .bind(list_item, "accessible-label", Widget::NONE);
                    video_item.connect_closure(
                        "play-video",
                        false,
                        closure_local!(
                            #[weak]
                            obj,
                            move |_: VideoItem, video: Video, play_type: PlayType| {
                                obj.emit_by_name::<()>("play-video", &[&video, &play_type]);
                            }
                        ),
                    );
                    video_item.connect_closure(
                        "copy-video-url",
                        false,
                        closure_local!(
                            #[weak]
                            obj,
                            move |_: VideoItem, video: Video| {
                                obj.emit_by_name::<()>("copy-video-url", &[&video]);
                            }
                        ),
                    );
                    video_item.connect_closure(
                        "download",
                        false,
                        closure_local!(
                            #[weak]
                            obj,
                            move |_: VideoItem, video: Video| {
                                obj.emit_by_name::<()>("download", &[&video]);
                            }
                        ),
                    );
                    video_item.connect_closure(
                        "remove-downloaded",
                        false,
                        closure_local!(
                            #[weak]
                            obj,
                            move |_: VideoItem, video: Video| {
                                obj.emit_by_name::<()>("remove-downloaded", &[&video]);
                            }
                        ),
                    );
                }
            ));
            // Downcasting the ListItem causes a segfault
            // Instead we must use this non-typesafe pattern
            factory.connect_bind(clone!(
                #[weak]
                obj,
                move |_, list_item: &Object| {
                    let widget = list_item
                        .property::<VideoItem>("child")
                        .property::<Widget>("parent");
                    obj.bind_property("compact", &widget, "halign")
                        .transform_to(|_, val: bool| {
                            Some(if val {
                                gtk::Align::Fill
                            } else {
                                gtk::Align::Center
                            })
                        })
                        .sync_create()
                        .build();
                }
            ));
            self.feed_list.set_factory(Some(&factory));

            self.feed_list.set_single_click_activate(true);
            self.feed_list.connect_activate(clone!(
                #[weak]
                obj,
                move |list_view, position| {
                    let model = list_view.model().expect("The model has to exist.");
                    let video = model
                        .item(position)
                        .expect("The item has to exist.")
                        .downcast::<Video>()
                        .expect("The item has to be an `Video`.");

                    obj.emit_by_name::<()>("play-video", &[&video, &PlayType::Default]);
                }
            ));

            self.obj().setup_autoload();
        }
    }

    #[gtk::template_callbacks]
    impl VideoList {
        #[template_callback]
        fn edge_reached(&self, pos: PositionType) {
            if pos == PositionType::Bottom {
                let _ = gtk::prelude::WidgetExt::activate_action(
                    self.obj().as_ref(),
                    "feed.more",
                    None,
                );
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VideoList {
        const NAME: &'static str = "TFVideoList";
        type Type = super::VideoList;
        type ParentType = adw::BreakpointBin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for VideoList {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().add_actions();
            self.setup();

            // Set the video stream to some empty dummy.
            // If it would be set to None, UI elements would not be able to check whether it is empty, and therefore display the wrong status pages.
            self.set_model(VideoFetcherStream::empty());
        }

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

    impl WidgetImpl for VideoList {}
    impl BreakpointBinImpl for VideoList {}
}
