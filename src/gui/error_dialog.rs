use adw::{ExpanderRow, prelude::ExpanderRowExt};
use gdk::{glib::Object, prelude::DisplayExt};
use glib::{clone, subclass::types::ObjectSubclassIsExt};
use gtk::{
    Align, Button, Label,
    prelude::{ButtonExt, WidgetExt},
};

gtk::glib::wrapper! {
    pub struct ErrorDialog(ObjectSubclass<imp::ErrorDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl ErrorDialog {
    pub fn new(errors: &[(String, Box<dyn std::error::Error>)]) -> Self {
        let s: Self = Object::builder().build();
        for (title, error) in errors {
            s.add(title, &**error);
        }
        s
    }

    fn add(&self, title: &String, error: &dyn std::error::Error) {
        let error_box = &self.imp().error_box;

        let display = format!("{error}");
        let debug = format!("{error:#?}");

        let error_label = Label::builder()
            .label(&debug)
            .wrap(true)
            .wrap_mode(gdk::pango::WrapMode::WordChar)
            .build();
        let expander_row = ExpanderRow::builder()
            .title(title)
            .subtitle(&display)
            .subtitle_lines(2)
            .build();

        let copy_button = Button::from_icon_name("edit-copy-symbolic");
        copy_button.set_valign(Align::Center);
        copy_button.add_css_class("flat");
        copy_button.connect_clicked(clone!(
            #[weak(rename_to = s)]
            self,
            #[strong]
            title,
            move |_| {
                let clipboard = s.display().clipboard();
                clipboard.set_text(&format!("{title}\n\n{display}\n\n{debug}"));
            }
        ));

        expander_row.add_row(&error_label);
        expander_row.add_suffix(&copy_button);

        error_box.append(&expander_row);
    }
}

pub mod imp {
    use adw::prelude::AdwDialogExt;
    use adw::subclass::prelude::AdwDialogImpl;
    use glib::subclass::InitializingObject;
    use gtk::CompositeTemplate;
    use gtk::ListBox;
    use gtk::glib;
    use gtk::prelude::WidgetExt;
    use gtk::subclass::prelude::*;
    use gtk::template_callbacks;

    #[derive(CompositeTemplate)]
    #[template(resource = "/ui/error_dialog.ui")]
    pub struct ErrorDialog {
        #[template_child]
        pub(super) error_box: TemplateChild<ListBox>,
    }

    #[template_callbacks]
    impl ErrorDialog {
        #[template_callback]
        fn clear_errors(&self) {
            let obj = self.obj();
            let _ = WidgetExt::activate_action(obj.as_ref(), "win.clear-errors", None);
            obj.close();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ErrorDialog {
        const NAME: &'static str = "TFErrorDialog";
        type Type = super::ErrorDialog;
        type ParentType = adw::Dialog;

        fn new() -> Self {
            Self {
                error_box: Default::default(),
            }
        }

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ErrorDialog {}
    impl WidgetImpl for ErrorDialog {}
    impl AdwDialogImpl for ErrorDialog {}
}
