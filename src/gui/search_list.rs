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

use super::window::Window;

const LOAD_COUNT: usize = 10;

gtk::glib::wrapper! {
    pub struct SearchList(ObjectSubclass<imp::SearchList>)
        @extends adw::BreakpointBin, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

impl SearchList {
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

        let actions = SimpleActionGroup::new();
        self.insert_action_group("search", Some(&actions));
        actions.add_action(&action_more);
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
                    let _ = s.activate_action("search.more", None);
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

    use crate::backend::Channel;
    use crate::backend::Client;
    use crate::backend::PlayType;
    use crate::backend::SearchItem;
    use crate::backend::SearchStream;
    use crate::backend::Video;
    use crate::gui::search_list_item::SearchListItem;
    use crate::gui::utility::Utility;

    use super::LOAD_COUNT;

    #[derive(CompositeTemplate, Properties, Default)]
    #[template(resource = "/ui/search_list.ui")]
    #[properties(wrapper_type = super::SearchList)]
    pub struct SearchList {
        #[template_child]
        pub(super) list: TemplateChild<gtk::GridView>,
        #[template_child]
        pub(super) scrolled_window: TemplateChild<gtk::ScrolledWindow>,

        #[property(get, set = Self::set_model)]
        model: RefCell<Option<SearchStream>>,
        #[property(get, set, nullable)]
        selected: RefCell<Option<Video>>,
        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,

        #[property(get, set)]
        client: RefCell<Client>,
    }

    impl SearchList {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);

            if compact {
                self.list.set_max_columns(1);
            } else {
                self.list.set_max_columns(6);
            }
        }

        fn set_model(&self, model: SearchStream) {
            let obj = self.obj();

            model.load_more(LOAD_COUNT);
            self.list.get().set_model(Some(&model));
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
                    let item = SearchListItem::new();
                    list_item.set_child(Some(&item));

                    list_item
                        .property_expression("item")
                        .bind(&item, "item", Widget::NONE);
                    obj.property_expression("client")
                        .bind(&item, "client", Widget::NONE);
                    obj.property_expression("compact")
                        .bind(&item, "compact", Widget::NONE);
                    item.property_expression("item")
                        .chain_closure::<Option<String>>(closure!(
                            |_: Option<Object>, item: Option<SearchItem>| {
                                item.map(Utility::accessible_description_for_search_item)
                            }
                        ))
                        .bind(list_item, "accessible-label", Widget::NONE);
                    item.connect_closure(
                        "play-video",
                        false,
                        closure_local!(
                            #[weak]
                            obj,
                            move |_: SearchListItem, video: Video, play_type: PlayType| {
                                obj.emit_by_name::<()>("play-video", &[&video, &play_type]);
                            }
                        ),
                    );
                    item.connect_closure(
                        "copy-video-url",
                        false,
                        closure_local!(
                            #[weak]
                            obj,
                            move |_: SearchListItem, video: Video| {
                                obj.emit_by_name::<()>("copy-video-url", &[&video]);
                            }
                        ),
                    );
                    item.connect_closure(
                        "download",
                        false,
                        closure_local!(
                            #[weak]
                            obj,
                            move |_: SearchListItem, video: Video| {
                                obj.emit_by_name::<()>("download", &[&video]);
                            }
                        ),
                    );
                    item.connect_closure(
                        "remove-downloaded",
                        false,
                        closure_local!(
                            #[weak]
                            obj,
                            move |_: SearchListItem, video: Video| {
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
                        .property::<SearchListItem>("child")
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
                    let item = model
                        .item(position)
                        .expect("The item has to exist.")
                        .downcast::<SearchItem>()
                        .expect("The item has to be an `Channel`.");

                    if let Some(video) = item.video() {
                        obj.emit_by_name::<()>("play-video", &[&video, &PlayType::Default]);
                    } else if let Some(channel) = item.channel() {
                        obj.emit_by_name::<()>("display-channel", &[&channel]);
                    } else {
                        log::warn!("Search page activated something, which is neither a video nor a channel. Doing nothing.")
                    }

                }
            ));

            self.obj().setup_autoload();
        }
    }

    #[gtk::template_callbacks]
    impl SearchList {
        #[template_callback]
        fn edge_reached(&self, pos: PositionType) {
            if pos == PositionType::Bottom {
                let _ = gtk::prelude::WidgetExt::activate_action(
                    self.obj().as_ref(),
                    "search.more",
                    None,
                );
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SearchList {
        const NAME: &'static str = "TFSearchList";
        type Type = super::SearchList;
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
    impl ObjectImpl for SearchList {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().add_actions();
            self.setup();

            // Set the search stream to some empty dummy.
            // If it would be set to None, UI elements would not be able to check whether it is empty, and therefore display the wrong status pages.
            self.set_model(SearchStream::empty());
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("play-video")
                        .param_types([Video::static_type(), PlayType::static_type()])
                        .build(),
                    Signal::builder("display-channel")
                        .param_types([Channel::static_type()])
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

    impl WidgetImpl for SearchList {}
    impl BreakpointBinImpl for SearchList {}
}
