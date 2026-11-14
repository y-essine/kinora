//! Kinora: a titled, horizontally scrolling row of video cards.
//!
//! One of Kinora's reusable building blocks (see `docs/design.md`): Home is
//! made of these, and rows contributed by extensions are rendered by this same
//! widget, so they cannot look out of place. Give it a title and any
//! `gio::ListModel` of [`Video`]s; it only builds the cards that are on screen.

gtk::glib::wrapper! {
    pub struct VideoRow(ObjectSubclass<imp::VideoRow>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl VideoRow {
    pub fn new(title: &str, client: &crate::backend::Client) -> Self {
        gtk::glib::Object::builder()
            .property("title", title)
            .property("client", client)
            .build()
    }
}

pub mod imp {
    use adw::prelude::*;
    use gdk::gio;
    use glib::Object;
    use glib::Properties;
    use glib::clone;
    use glib::closure;
    use glib::closure_local;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::CompositeTemplate;
    use gtk::SignalListItemFactory;
    use gtk::Widget;
    use gtk::glib;
    use gtk::subclass::prelude::*;
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::sync::LazyLock;

    use crate::backend::Client;
    use crate::backend::PlayType;
    use crate::backend::Video;
    use crate::config::BASE_ID;
    use crate::gui::utility::Utility;
    use crate::gui::video_item::VideoItem;

    #[derive(CompositeTemplate, Properties)]
    #[template(resource = "/ui/video_row.ui")]
    #[properties(wrapper_type = super::VideoRow)]
    pub struct VideoRow {
        #[template_child]
        list: TemplateChild<gtk::ListView>,
        #[template_child]
        scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        stack: TemplateChild<gtk::Stack>,
        #[template_child]
        placeholder: TemplateChild<gtk::Box>,
        #[template_child]
        btn_prev: TemplateChild<gtk::Button>,
        #[template_child]
        btn_next: TemplateChild<gtk::Button>,

        #[property(get, set)]
        title: RefCell<String>,
        #[property(get, set)]
        subtitle: RefCell<String>,
        /// Any list of `Video`s. The row hides itself while it is empty.
        #[property(get, set = Self::set_model, nullable)]
        model: RefCell<Option<gio::ListModel>>,
        /// The videos are on their way: show quiet placeholders meanwhile.
        #[property(get, set = Self::set_loading)]
        loading: Cell<bool>,
        /// Width of the cards; their thumbnails are 16:9.
        #[property(get, set = Self::set_card_width, default = 272)]
        card_width: Cell<i32>,
        #[property(get, set)]
        client: RefCell<Client>,

        items_changed: RefCell<Option<(gio::ListModel, glib::SignalHandlerId)>>,
        animation: RefCell<Option<adw::TimedAnimation>>,
        settings: gio::Settings,
    }

    impl Default for VideoRow {
        fn default() -> Self {
            Self {
                list: Default::default(),
                scroller: Default::default(),
                stack: Default::default(),
                placeholder: Default::default(),
                btn_prev: Default::default(),
                btn_next: Default::default(),
                title: Default::default(),
                subtitle: Default::default(),
                model: Default::default(),
                loading: Default::default(),
                card_width: Cell::new(272),
                client: Default::default(),
                items_changed: Default::default(),
                animation: Default::default(),
                settings: gio::Settings::new(BASE_ID),
            }
        }
    }

    #[gtk::template_callbacks]
    impl VideoRow {
        fn set_model(&self, model: Option<gio::ListModel>) {
            if let Some((old, handler)) = self.items_changed.take() {
                old.disconnect(handler);
            }
            if let Some(model) = &model {
                let handler = model.connect_items_changed(clone!(
                    #[weak(rename_to = s)]
                    self,
                    move |_, _, _, _| s.update_state()
                ));
                self.items_changed.replace(Some((model.clone(), handler)));
            }
            self.list.set_model(
                model
                    .as_ref()
                    .map(|m| gtk::NoSelection::new(Some(m.clone())))
                    .as_ref(),
            );
            self.model.replace(model);
            self.update_state();
        }

        fn set_loading(&self, loading: bool) {
            self.loading.set(loading);
            self.update_state();
        }

        fn set_card_width(&self, width: i32) {
            self.card_width.set(width.max(120));
            self.fill_placeholder();
        }

        fn n_items(&self) -> u32 {
            self.model.borrow().as_ref().map_or(0, |m| m.n_items())
        }

        /// Empty rows take no room; loading ones show placeholders.
        fn update_state(&self) {
            let empty = self.n_items() == 0;
            let loading = self.loading.get();
            self.obj().set_visible(!empty || loading);
            self.stack.set_visible_child_name(if empty && loading {
                "placeholder"
            } else {
                "list"
            });
            self.update_nav();
        }

        fn fill_placeholder(&self) {
            while let Some(child) = self.placeholder.first_child() {
                self.placeholder.remove(&child);
            }
            let width = self.card_width.get();
            for _ in 0..6 {
                let block = gtk::Box::builder()
                    .css_classes(["k-skeleton"])
                    .width_request(width)
                    .height_request((width as f64 * 9.0 / 16.0).round() as i32)
                    .valign(gtk::Align::Start)
                    .build();
                self.placeholder.append(&block);
            }
        }

        /// The arrows are only there when there is somewhere to scroll to.
        fn update_nav(&self) {
            let adj = self.scroller.hadjustment();
            let scrollable = adj.upper() - adj.lower() > adj.page_size() + 1.0
                && self.stack.visible_child_name().as_deref() == Some("list");
            self.btn_prev.set_visible(scrollable);
            self.btn_next.set_visible(scrollable);
            self.btn_prev.set_sensitive(adj.value() > adj.lower() + 1.0);
            self.btn_next
                .set_sensitive(adj.value() < adj.upper() - adj.page_size() - 1.0);
        }

        fn scroll_by_page(&self, direction: f64) {
            let adj = self.scroller.hadjustment();
            let step = (adj.page_size() * 0.85).max(self.card_width.get() as f64);
            let target = (adj.value() + direction * step).clamp(
                adj.lower(),
                (adj.upper() - adj.page_size()).max(adj.lower()),
            );
            if let Some(animation) = self.animation.take() {
                animation.skip();
            }
            if self.settings.boolean("reduce-effects") {
                adj.set_value(target);
                return;
            }
            let animation = adw::TimedAnimation::builder()
                .widget(&*self.scroller)
                .value_from(adj.value())
                .value_to(target)
                .duration(250)
                .easing(adw::Easing::EaseOutCubic)
                .target(&adw::PropertyAnimationTarget::new(&adj, "value"))
                .build();
            animation.play();
            self.animation.replace(Some(animation));
        }

        #[template_callback]
        fn scroll_back(&self) {
            self.scroll_by_page(-1.0);
        }

        #[template_callback]
        fn scroll_forward(&self) {
            self.scroll_by_page(1.0);
        }

        #[template_callback(function)]
        fn has_text(text: &str) -> bool {
            !text.is_empty()
        }

        fn setup(&self) {
            let obj = self.obj();
            self.fill_placeholder();

            let adj = self.scroller.hadjustment();
            adj.connect_changed(clone!(
                #[weak(rename_to = s)]
                self,
                move |_| s.update_nav()
            ));
            adj.connect_value_changed(clone!(
                #[weak(rename_to = s)]
                self,
                move |adj| {
                    s.update_nav();
                    // Close to the end: rows that can load more do so now.
                    let rest = adj.upper() - adj.page_size() - adj.value();
                    if rest < 2.0 * s.card_width.get() as f64 {
                        s.obj().emit_by_name::<()>("scroll-end", &[]);
                    }
                }
            ));

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
                    obj.property_expression("card-width").bind(
                        &video_item,
                        "card-width",
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
                    for signal in ["copy-video-url", "download", "remove-downloaded"] {
                        video_item.connect_closure(
                            signal,
                            false,
                            closure_local!(
                                #[weak]
                                obj,
                                move |_: VideoItem, video: Video| {
                                    obj.emit_by_name::<()>(signal, &[&video]);
                                }
                            ),
                        );
                    }
                }
            ));
            self.list.set_factory(Some(&factory));
            self.list.connect_activate(clone!(
                #[weak]
                obj,
                move |list, position| {
                    if let Some(video) = list
                        .model()
                        .and_then(|m| m.item(position))
                        .and_downcast::<Video>()
                    {
                        obj.emit_by_name::<()>("play-video", &[&video, &PlayType::Default]);
                    }
                }
            ));
            self.update_state();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VideoRow {
        const NAME: &'static str = "TFVideoRow";
        type Type = super::VideoRow;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for VideoRow {
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
                    // The row was scrolled close to its end.
                    Signal::builder("scroll-end").build(),
                ]
            });
            SIGNALS.as_ref()
        }

        fn dispose(&self) {
            if let Some((model, handler)) = self.items_changed.take() {
                model.disconnect(handler);
            }
        }
    }

    impl WidgetImpl for VideoRow {}
    impl BoxImpl for VideoRow {}
}
