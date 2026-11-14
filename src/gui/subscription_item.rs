use gtk::glib::Object;
gtk::glib::wrapper! {
    pub struct SubscriptionItem(ObjectSubclass<imp::SubscriptionItem>)
        @extends adw::ActionRow, adw::PreferencesRow, gtk::ListBoxRow, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Actionable;
}

impl Default for SubscriptionItem {
    fn default() -> Self {
        Self::new()
    }
}

impl SubscriptionItem {
    pub fn new() -> Self {
        let s: Self = Object::builder::<Self>().build();
        s
    }
}

pub mod imp {
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::sync::LazyLock;

    use adw::subclass::action_row::ActionRowImpl;
    use adw::subclass::preferences_row::PreferencesRowImpl;
    use gdk::gio::SimpleAction;
    use gdk::gio::SimpleActionGroup;
    use glib::Properties;
    use glib::clone;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::CompositeTemplate;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    use crate::backend::Channel;
    use crate::backend::Client;
    use crate::gui::utility::Utility;

    #[derive(CompositeTemplate, Default, Properties)]
    #[template(resource = "/ui/subscription_item.ui")]
    #[properties(wrapper_type = super::SubscriptionItem)]
    pub struct SubscriptionItem {
        #[template_child]
        unsub_button_stack: TemplateChild<gtk::Stack>,

        #[property(get, set)]
        channel: RefCell<Option<Channel>>,

        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,
        #[property(get, set)]
        client: RefCell<Client>,
    }

    #[gtk::template_callbacks]
    impl SubscriptionItem {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);

            if compact {
                self.unsub_button_stack.set_visible_child_name("compact");
            } else {
                self.unsub_button_stack.set_visible_child_name("wide");
            }
        }

        fn setup_actions(&self) {
            let obj = self.obj();
            let action_unsubscribe_and_show_toast =
                SimpleAction::new("unsubscribe-and-show-toast", None);
            action_unsubscribe_and_show_toast.connect_activate(clone!(
                #[strong]
                obj,
                move |_, _| {
                    if let Some(channel) = obj.channel() {
                        obj.emit_by_name::<()>("show-unsubscribe-toast", &[&channel]);
                        obj.client().toggle_subscription(channel);
                    }
                }
            ));

            let actions = SimpleActionGroup::new();
            obj.insert_action_group("subscription-item", Some(&actions));
            actions.add_action(&action_unsubscribe_and_show_toast);
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SubscriptionItem {
        const NAME: &'static str = "TFSubscriptionItem";
        type Type = super::SubscriptionItem;
        type ParentType = adw::ActionRow;

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
    impl ObjectImpl for SubscriptionItem {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup_actions();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("show-unsubscribe-toast")
                        .param_types([Channel::static_type()])
                        .build(),
                ]
            });
            SIGNALS.as_ref()
        }
    }

    impl WidgetImpl for SubscriptionItem {}
    impl ListBoxRowImpl for SubscriptionItem {}
    impl PreferencesRowImpl for SubscriptionItem {}
    impl ActionRowImpl for SubscriptionItem {}
}
