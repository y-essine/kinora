gtk::glib::wrapper! {
    pub struct SubscriptionsList(ObjectSubclass<imp::SubscriptionsList>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

pub mod imp {
    use adw::subclass::bin::BinImpl;
    use glib::Properties;
    use glib::clone;
    use glib::closure_local;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::ListBox;
    use gtk::ListView;
    use gtk::SelectionModel;
    use gtk::SignalListItemFactory;
    use gtk::Stack;
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
    use crate::backend::SubscriptionsStore;
    use crate::gui::subscription_item::SubscriptionItem;

    static BULK_CUTOFF: u32 = 250;

    #[derive(CompositeTemplate, Properties, Default)]
    #[template(resource = "/ui/subscriptions_list.ui")]
    #[properties(wrapper_type = super::SubscriptionsList)]
    pub struct SubscriptionsList {
        #[template_child]
        list: TemplateChild<ListBox>,
        #[template_child]
        grid: TemplateChild<ListView>,
        #[template_child]
        stack: TemplateChild<Stack>,

        #[property(get, set = Self::set_model)]
        model: RefCell<Option<SubscriptionsStore>>,
        #[property(get, set, nullable)]
        selected: RefCell<Option<Channel>>,
        #[property(get, set)]
        compact: Cell<bool>,

        #[property(get, set)]
        client: RefCell<Client>,
    }

    impl SubscriptionsList {
        fn reset_model(&self) {
            let obj = self.obj();
            let model = self
                .model
                .borrow()
                .clone()
                .expect("Subscription model to be set");
            // For many subscriptions, the ListBox is very inefficient, fall back to a Grid.
            if model.n_items() < BULK_CUTOFF {
                self.stack.set_visible_child_name("normal");
                self.grid.set_model(None::<&SelectionModel>);
                self.list.bind_model(
                    Some(&model),
                    clone!(
                        #[strong]
                        obj,
                        move |channel| {
                            let channel = channel
                                .dynamic_cast_ref::<Channel>()
                                .expect("SubscriptionsStore to only contain channels");
                            let subscription_item = SubscriptionItem::new();
                            subscription_item.set_channel(channel);
                            subscription_item.connect_closure(
                                "show-unsubscribe-toast",
                                false,
                                closure_local!(
                                    #[weak]
                                    obj,
                                    move |_: SubscriptionItem, channel: Channel| {
                                        obj.emit_by_name::<()>(
                                            "show-unsubscribe-toast",
                                            &[&channel],
                                        );
                                    }
                                ),
                            );
                            obj.property_expression("client").bind(
                                &subscription_item,
                                "client",
                                Widget::NONE,
                            );
                            obj.property_expression("compact").bind(
                                &subscription_item,
                                "compact",
                                Widget::NONE,
                            );
                            subscription_item.into()
                        }
                    ),
                );
            } else {
                self.stack.set_visible_child_name("many-subscriptions");
                self.grid.set_model(Some(&model));
                self.list.unbind_model();

                let factory = SignalListItemFactory::new();
                factory.connect_setup(clone!(
                    #[weak]
                    obj,
                    move |_, list_item| {
                        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
                        let subscription_item = SubscriptionItem::new();
                        list_item.set_child(Some(&subscription_item));

                        list_item.property_expression("item").bind(
                            &subscription_item,
                            "channel",
                            Widget::NONE,
                        );
                        subscription_item.connect_closure(
                            "show-unsubscribe-toast",
                            false,
                            closure_local!(
                                #[weak]
                                obj,
                                move |_: SubscriptionItem, channel: Channel| {
                                    obj.emit_by_name::<()>("show-unsubscribe-toast", &[&channel]);
                                }
                            ),
                        );
                        obj.property_expression("client").bind(
                            &subscription_item,
                            "client",
                            Widget::NONE,
                        );
                        obj.property_expression("compact").bind(
                            &subscription_item,
                            "compact",
                            Widget::NONE,
                        );
                    }
                ));
                self.grid.set_factory(Some(&factory));
            }
        }

        fn set_model(&self, model: SubscriptionsStore) {
            let obj = self.obj();
            obj.property_expression("selected")
                .bind(&model, "selected", Widget::NONE);
            self.model.replace(Some(model.clone()));

            model.connect_items_changed(clone!(
                #[weak]
                obj,
                move |model, _idx, removed, added| {
                    let n_items = model.n_items();
                    if (n_items >= BULK_CUTOFF && n_items - added < BULK_CUTOFF)
                        || (n_items < BULK_CUTOFF && n_items + removed >= BULK_CUTOFF)
                    {
                        obj.imp().reset_model();
                    }
                }
            ));

            self.reset_model();
        }

        pub(super) fn setup(&self) {
            let obj = self.obj();

            self.list.connect_row_activated(clone!(
                #[strong]
                obj,
                move |_list_view, row| {
                    let subscription_item = row
                        .dynamic_cast_ref::<SubscriptionItem>()
                        .expect("Row of subscription list to be a subscription item");
                    let channel = subscription_item.channel();
                    obj.emit_by_name::<()>("display-channel", &[&channel]);
                }
            ));

            self.grid.connect_activate(clone!(
                #[strong]
                obj,
                move |_list_view, row| {
                    let channel = obj
                        .model()
                        .expect("Subscription list to have a model")
                        .get(row as usize);
                    obj.emit_by_name::<()>("display-channel", &[&channel]);
                }
            ));
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SubscriptionsList {
        const NAME: &'static str = "TFSubscriptionsList";
        type Type = super::SubscriptionsList;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for SubscriptionsList {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("display-channel")
                        .param_types([Channel::static_type()])
                        .build(),
                    Signal::builder("show-unsubscribe-toast")
                        .param_types([Channel::static_type()])
                        .build(),
                ]
            });
            SIGNALS.as_ref()
        }
    }

    impl WidgetImpl for SubscriptionsList {}
    impl BinImpl for SubscriptionsList {}
}
