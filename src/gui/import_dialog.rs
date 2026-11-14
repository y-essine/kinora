use gdk::glib::Object;
use glib::object::Cast;
use gtk::prelude::WidgetExt;

use crate::backend::Client;

use super::window::Window;

gtk::glib::wrapper! {
    pub struct ImportDialog(ObjectSubclass<imp::ImportDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl ImportDialog {
    pub fn new(client: Client) -> Self {
        Object::builder().property("client", client).build()
    }

    fn window(&self) -> Window {
        self.root().unwrap().dynamic_cast().unwrap()
    }
}

pub mod imp {
    use std::cell::RefCell;

    use adw::Toast;
    use adw::prelude::AdwDialogExt;
    use adw::subclass::prelude::AdwDialogImpl;
    use gdk::gio::Cancellable;
    use glib::subclass::InitializingObject;
    use glib::{Properties, clone};
    use gtk::prelude::{ObjectExt, WidgetExt};
    use gtk::subclass::prelude::*;
    use gtk::{Button, CompositeTemplate, FileDialog, FileFilter, Stack};
    use gtk::{glib, template_callbacks};

    use crate::backend::Client;

    #[derive(CompositeTemplate, Default, Properties)]
    #[properties(wrapper_type = super::ImportDialog)]
    #[template(resource = "/ui/import_dialog.ui")]
    pub struct ImportDialog {
        #[template_child]
        stack: TemplateChild<Stack>,
        #[template_child]
        button: TemplateChild<Button>,
        #[property(get, set)]
        client: RefCell<Client>,
    }

    #[template_callbacks]
    impl ImportDialog {
        fn make_json_file_filter() -> FileFilter {
            let json_filter = FileFilter::new();
            json_filter.add_mime_type("application/json");
            json_filter.add_mime_type("application/vnd.document+json");
            json_filter
        }

        #[template_callback]
        fn import_pipeline(&self) {
            let obj = self.obj();
            let window = obj.window();

            let file_dialog = FileDialog::builder()
                .accept_label(gettextrs::gettext("Import"))
                .title(gettextrs::gettext("Import Pipeline Data"))
                .build();
            file_dialog.open(
                Some(&window),
                None::<&Cancellable>,
                clone!(
                    #[weak]
                    obj,
                    #[weak]
                    window,
                    move |res| {
                        if let Ok(file) = res {
                            if let Err(e) = obj.client().restore(file) {
                                window.display_error(
                                    gettextrs::gettext("Error importing Pipeline Data"),
                                    e,
                                );
                            } else {
                                window.show_toast(Toast::new(&gettextrs::gettext(
                                    "Successfully imported data. A restart of the application is recommended.",
                                )));
                            }
                            obj.close();
                        }
                    }
                ),
            )
        }

        #[template_callback]
        fn import_youtube(&self) {
            let obj = self.obj();
            let window = obj.window();

            let csv_filter = FileFilter::new();
            csv_filter.add_mime_type("text/csv");
            let file_dialog = FileDialog::builder()
                .accept_label(gettextrs::gettext("Import"))
                .default_filter(&csv_filter)
                .title(gettextrs::gettext("Import YouTube Subscriptions"))
                .build();
            file_dialog.open(
                Some(&window),
                None::<&Cancellable>,
                clone!(
                    #[weak]
                    obj,
                    #[weak]
                    window,
                    move |res| {
                        if let Ok(file) = res {
                            if let Err(e) = obj.client().import_youtube(file) {
                                window.display_error(
                                    gettextrs::gettext("Error importing YouTube subscriptions"),
                                    e,
                                );
                            } else {
                                window.show_toast(Toast::new(&gettextrs::gettext(
                                    "Successfully imported subscriptions",
                                )));
                            }
                            obj.close();
                        }
                    }
                ),
            )
        }

        #[template_callback]
        fn import_newpipe(&self) {
            let obj = self.obj();
            let window = obj.window();

            let json_filter = Self::make_json_file_filter();
            let file_dialog = FileDialog::builder()
                .accept_label(gettextrs::gettext("Import"))
                .default_filter(&json_filter)
                .title(gettextrs::gettext("Import NewPipe Subscriptions"))
                .build();
            file_dialog.open(
                Some(&window),
                None::<&Cancellable>,
                clone!(
                    #[weak]
                    obj,
                    #[weak]
                    window,
                    move |res| {
                        if let Ok(file) = res {
                            if let Err(e) = obj.client().import_newpipe(file) {
                                window.display_error(
                                    gettextrs::gettext("Error importing NewPipe subscriptions"),
                                    e,
                                );
                            } else {
                                window.show_toast(Toast::new(&gettextrs::gettext(
                                    "Successfully imported subscriptions",
                                )));
                            }
                            obj.close();
                        }
                    }
                ),
            )
        }

        #[template_callback]
        fn import_invidious(&self) {
            let obj = self.obj();
            let window = obj.window();

            let json_filter = Self::make_json_file_filter();
            let file_dialog = FileDialog::builder()
                .accept_label(gettextrs::gettext("Import"))
                .default_filter(&json_filter)
                .title(gettextrs::gettext("Import Invidious Subscriptions"))
                .build();
            file_dialog.open(
                Some(&window),
                None::<&Cancellable>,
                clone!(
                    #[weak]
                    obj,
                    #[weak]
                    window,
                    move |res| {
                        if let Ok(file) = res {
                            if let Err(e) = obj.client().import_invidious(file) {
                                window.display_error(
                                    gettextrs::gettext("Error importing Invidious subscriptions"),
                                    e,
                                );
                            } else {
                                window.show_toast(Toast::new(&gettextrs::gettext(
                                    "Successfully imported subscriptions",
                                )));
                            }
                            obj.close();
                        }
                    }
                ),
            )
        }

        #[template_callback]
        fn import(&self) {
            match self.stack.visible_child_name().as_deref() {
                Some("YouTube") => self.import_youtube(),
                Some("Pipeline") => self.import_pipeline(),
                Some("NewPipe") => self.import_newpipe(),
                Some("Invidious") => self.import_invidious(),
                _ => log::warn!("Unknown import format; doing nothing"),
            }
        }

        #[template_callback(function)]
        fn get_format(object: &gtk::StringObject) -> glib::GString {
            object.string()
        }

        fn setup(&self) {
            self.stack.connect_visible_child_name_notify(clone!(
                #[strong(rename_to=obj)]
                self.obj(),
                move |stack| {
                    let button = &obj.imp().button;
                    if stack.visible_child_name().as_deref() == Some("Pipeline") {
                        button.add_css_class("destructive-action");
                        button.remove_css_class("suggested-action");
                    } else {
                        button.add_css_class("suggested-action");
                        button.remove_css_class("destructive-action");
                    }
                }
            ));
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ImportDialog {
        const NAME: &'static str = "TFImportDialog";
        type Type = super::ImportDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for ImportDialog {
        fn constructed(&self) {
            self.parent_constructed();
            self.setup();
        }
    }
    impl WidgetImpl for ImportDialog {}
    impl AdwDialogImpl for ImportDialog {}
}
