gtk::glib::wrapper! {
    pub struct FiniteVideoList(ObjectSubclass<imp::FiniteVideoList>)
        @extends adw::BreakpointBin, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

pub mod imp {
    use adw::subclass::breakpoint_bin::BreakpointBinImpl;
    use glib::Object;
    use glib::Properties;
    use glib::clone;
    use glib::closure;
    use glib::closure_local;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::GridView;
    use gtk::SignalListItemFactory;
    use gtk::Widget;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::sync::LazyLock;

    use gtk::CompositeTemplate;

    use crate::backend::Client;
    use crate::backend::FiniteVideoStore;
    use crate::backend::PlayType;
    use crate::backend::Video;
    use crate::gui::utility::Utility;
    use crate::gui::video_item::VideoItem;

    #[derive(CompositeTemplate, Properties, Default)]
    #[template(resource = "/ui/finite_video_list.ui")]
    #[properties(wrapper_type = super::FiniteVideoList)]
    pub struct FiniteVideoList {
        #[template_child]
        list: TemplateChild<GridView>,

        #[property(get, set = Self::set_model)]
        model: RefCell<Option<FiniteVideoStore>>,
        #[property(get, set, nullable)]
        selected: RefCell<Option<Video>>,
        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,

        #[property(get, set)]
        client: RefCell<Client>,
    }

    impl FiniteVideoList {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);

            if compact {
                self.list.set_max_columns(1);
            } else {
                self.list.set_max_columns(6);
            }
        }

        fn set_model(&self, model: FiniteVideoStore) {
            self.obj()
                .property_expression("selected")
                .bind(&model, "selected", Widget::NONE);
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
                    list_item.set_child(Some(&video_item));

                    list_item
                        .property_expression("item")
                        .bind(&video_item, "video", Widget::NONE);
                    obj.property_expression("client")
                        .bind(&video_item, "client", Widget::NONE);
                    obj.property_expression("compact")
                        .bind(&video_item, "compact", Widget::NONE);
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
            self.list.set_factory(Some(&factory));

            self.list.set_single_click_activate(true);
            self.list.connect_activate(clone!(
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
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FiniteVideoList {
        const NAME: &'static str = "TFFiniteVideoList";
        type Type = super::FiniteVideoList;
        type ParentType = adw::BreakpointBin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for FiniteVideoList {
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

    impl WidgetImpl for FiniteVideoList {}
    impl BreakpointBinImpl for FiniteVideoList {}
}
