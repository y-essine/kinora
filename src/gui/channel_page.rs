use glib::{Object, clone, object::Cast};
// Forward all signals from the video list.
use gtk::prelude::WidgetExt;

use crate::{
    backend::{Channel, Client},
    gspawn,
};

use super::window::Window;

gtk::glib::wrapper! {
    pub struct ChannelPage(ObjectSubclass<imp::ChannelPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget;
}

const LOAD_COUNT: usize = 10;

impl ChannelPage {
    pub fn new(channel: &Channel, client: &Client) -> Self {
        Object::builder::<Self>()
            .property("client", client)
            .property("channel", channel)
            .build()
    }

    pub fn reload(&self) {
        if let Some(channel) = self.channel() {
            let client = self.client();
            // Load additional data
            gspawn!(clone!(
                #[weak]
                client,
                #[weak]
                channel,
                #[weak(rename_to = s)]
                self,
                async move {
                    let result = client.populate_channel_information(&channel.id()).await;
                    if let Err(e) = result {
                        log::error!("Failed to populate channel information: {e}");
                        s.window().display_error(
                            gettextrs::gettext("Failed to load channel data"),
                            Box::new(e),
                        );
                    }

                    // Load videos
                    let videos = client.videos_for_channels(&[channel.id()], false).await;
                    videos.load_more(LOAD_COUNT);
                    s.set_model(videos);
                }
            ));
        }
    }

    fn window(&self) -> Window {
        self.root().unwrap().dynamic_cast().unwrap()
    }
}

pub mod imp {
    use std::cell::Cell;
    use std::cell::RefCell;

    use adw::MultiLayoutView;
    use adw::subclass::bin::BinImpl;
    use gdk::gio::SimpleAction;
    use gdk::gio::SimpleActionGroup;
    use glib::Properties;
    use glib::clone;
    use glib::subclass::InitializingObject;
    use glib::subclass::Signal;
    use gtk::Button;
    use gtk::CompositeTemplate;
    use gtk::Label;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use std::sync::LazyLock;

    use crate::backend::Channel;
    use crate::backend::Client;
    use crate::backend::PlayType;
    use crate::backend::Video;
    use crate::backend::VideoFetcherStream;
    use crate::gui::utility::Utility;
    use crate::gui::video_list::VideoList;

    #[derive(CompositeTemplate, Default, Properties)]
    #[template(resource = "/ui/channel_page.ui")]
    #[properties(wrapper_type = super::ChannelPage)]
    pub struct ChannelPage {
        #[template_child]
        box_main_area: TemplateChild<gtk::Box>,
        #[template_child]
        label_author: TemplateChild<Label>,
        #[template_child]
        button_subscribe: TemplateChild<Button>,
        #[template_child]
        multi_layout_view: TemplateChild<MultiLayoutView>,

        #[template_child]
        video_list: TemplateChild<VideoList>,

        #[property(get, set = Self::set_channel, nullable)]
        channel: RefCell<Option<Channel>>,

        #[property(get, set)]
        show_sidebar_visible: Cell<bool>,
        #[property(get, set)]
        primary_menu_visible: Cell<bool>,
        #[property(get, set = Self::set_compact)]
        compact: Cell<bool>,

        #[property(get, set)]
        model: RefCell<VideoFetcherStream>,

        #[property(get, set)]
        client: RefCell<Client>,
    }

    impl ChannelPage {
        fn set_compact(&self, compact: bool) {
            self.compact.set(compact);

            if compact {
                self.multi_layout_view.set_layout_name("compact");
            } else {
                self.multi_layout_view.set_layout_name("wide");
            }
        }

        // Recommended syntax is ugly and the item will be moved to `pedantic` soon.
        #[allow(clippy::assigning_clones)]
        fn set_channel(&self, channel: Option<Channel>) {
            *self.channel.borrow_mut() = channel.clone();
            self.obj().reload();
        }

        fn setup(&self) {
            let obj = self.obj();

            // Forward all signals from the video list.
            for signal in Self::signals() {
                let name = signal.name();
                self.video_list.connect_local(
                    name,
                    false,
                    clone!(
                        #[weak]
                        obj,
                        #[upgrade_or_default]
                        move |vals| {
                            // Argument 0 is the video list; strip that out.
                            obj.emit_by_name_with_values(name, &vals[1..]);
                            None
                        }
                    ),
                );
            }
        }

        fn add_actions(&self) {
            let obj = self.obj();
            let action_reload = SimpleAction::new("reload", None);
            action_reload.connect_activate(clone!(
                #[weak]
                obj,
                move |_, _| {
                    obj.reload();
                }
            ));

            let actions = SimpleActionGroup::new();
            obj.insert_action_group("channel-page", Some(&actions));
            actions.add_action(&action_reload);
        }
    }

    #[gtk::template_callbacks]
    impl ChannelPage {
        #[template_callback(function)]
        fn refresh_button_visible_child_name(loading: bool) -> &'static str {
            if loading { "spinner" } else { "button" }
        }

        #[template_callback]
        fn subscribe_label(&self, subscribed: bool) -> String {
            if subscribed {
                self.button_subscribe.remove_css_class("suggested-action");
                gettextrs::gettext("Unsubscribe")
            } else {
                self.button_subscribe.add_css_class("suggested-action");
                gettextrs::gettext("Subscribe")
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChannelPage {
        const NAME: &'static str = "TFChannelPage";
        type Type = super::ChannelPage;
        type ParentType = adw::Bin;

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
    impl ObjectImpl for ChannelPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup();
            self.add_actions();
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

    impl WidgetImpl for ChannelPage {
        fn grab_focus(&self) -> bool {
            self.label_author.select_region(0, 0);
            self.box_main_area.grab_focus()
        }
    }

    impl BinImpl for ChannelPage {}
}
