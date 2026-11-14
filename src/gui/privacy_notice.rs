use glib::Object;

glib::wrapper! {
    pub struct PrivacyNotice(ObjectSubclass<imp::PrivacyNotice>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Default for PrivacyNotice {
    fn default() -> Self {
        Self::new()
    }
}

impl PrivacyNotice {
    pub fn new() -> Self {
        Object::builder::<Self>().build()
    }
}

pub mod imp {
    use adw::prelude::AdwDialogExt;
    use adw::subclass::dialog::AdwDialogImpl;
    use glib::subclass::InitializingObject;
    use glib::subclass::object::ObjectImpl;
    use glib::subclass::types::ObjectSubclass;
    use gtk::CompositeTemplate;
    use gtk::subclass::prelude::*;
    use gtk::subclass::widget::WidgetImpl;

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/ui/privacy_notice.ui")]
    pub struct PrivacyNotice {}

    #[gtk::template_callbacks]
    impl PrivacyNotice {
        #[template_callback]
        fn handle_dismiss(&self) {
            self.obj().close();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PrivacyNotice {
        const NAME: &'static str = "TFPrivacyNotice";
        type Type = super::PrivacyNotice;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PrivacyNotice {}

    impl WidgetImpl for PrivacyNotice {}
    impl AdwDialogImpl for PrivacyNotice {}
}
