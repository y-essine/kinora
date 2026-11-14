use gtk::glib::Object;

gtk::glib::wrapper! {
    pub struct ChannelItem(ObjectSubclass<imp::ChannelItem>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

impl Default for ChannelItem {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelItem {
    pub fn new() -> Self {
        let s: Self = Object::builder::<Self>().build();
        s
    }
}

pub mod imp {
    use std::cell::Cell;
    use std::cell::RefCell;

    use adw::Avatar;
    use adw::MultiLayoutView;
    use gdk::Rectangle;
    use gdk::gio;
    use gdk::gio::SimpleAction;
    use gdk::gio::SimpleActionGroup;
    use glib::Properties;
    use glib::clone;
    use glib::subclass::InitializingObject;
    use gtk::CompositeTemplate;
    use gtk::Label;
    use gtk::PopoverMenu;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    use crate::backend::Channel;
    use crate::backend::Client;
    use crate::gui::utility::Utility;

    #[derive(CompositeTemplate, Default, Properties)]
    #[template(resource = "/ui/channel_item.ui")]
    #[properties(wrapper_type = super::ChannelItem)]
    pub struct ChannelItem {
        #[template_child]
        popover_menu: TemplateChild<PopoverMenu>,
        #[template_child]
        multi_layout_view: TemplateChild<MultiLayoutView>,
        #[template_child]
        avatar: TemplateChild<Avatar>,
        #[template_child]
        label_author: TemplateChild<Label>,

        #[property(get, set)]
        channel: RefCell<Option<Channel>>,
        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,

        #[property(get, set)]
        client: RefCell<Client>,
    }

    #[gtk::template_callbacks]
    impl ChannelItem {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);

            let obj = self.obj();

            if compact {
                self.multi_layout_view.set_layout_name("compact");
                obj.set_halign(gtk::Align::Fill);
                obj.set_margin_top(6);
                obj.set_margin_bottom(6);
                self.avatar.set_size(72);
                self.avatar.set_margin_bottom(0);
                self.label_author.remove_css_class("title-4");
                self.label_author.add_css_class("heading");
            } else {
                self.multi_layout_view.set_layout_name("wide");
                obj.set_halign(gtk::Align::Center);
                obj.set_margin_top(18);
                obj.set_margin_bottom(18);
                self.avatar.set_size(96);
                self.avatar.set_margin_bottom(12);
                self.label_author.add_css_class("title-4");
                self.label_author.remove_css_class("heading");
            }
        }

        #[template_callback]
        pub fn handle_right_clicked(&self, _n_presses: i32, x: f64, y: f64) {
            self.popover_menu.set_menu_model(Some(&self.create_menu()));
            self.popover_menu
                .set_pointing_to(Some(&Rectangle::new(x as i32, y as i32, 1, 1)));
            self.popover_menu.popup();
        }

        fn setup_actions(&self) {
            let obj = self.obj();
            let action_subscribe = SimpleAction::new("subscribe", None);
            action_subscribe.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    if let Some(channel) = obj.channel() {
                        obj.client().toggle_subscription(channel);
                    }
                }
            ));

            let actions = SimpleActionGroup::new();
            obj.insert_action_group("channel-item", Some(&actions));
            actions.add_action(&action_subscribe);
        }

        fn create_menu(&self) -> gio::Menu {
            let menu = gio::Menu::new();
            if self.obj().channel().is_some_and(|v| v.subscribed()) {
                menu.append(
                    Some(&gettextrs::gettext("Unsubscribe")),
                    Some("channel-item.subscribe"),
                );
            } else {
                menu.append(
                    Some(&gettextrs::gettext("Subscribe")),
                    Some("channel-item.subscribe"),
                );
            }

            menu
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChannelItem {
        const NAME: &'static str = "TFChannelItem";
        type Type = super::ChannelItem;
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
    impl ObjectImpl for ChannelItem {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup_actions();
        }
    }

    impl WidgetImpl for ChannelItem {}
    impl BoxImpl for ChannelItem {}
}
