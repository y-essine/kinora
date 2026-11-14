use gdk::glib::Object;
use glib::object::Cast;
use gtk::prelude::WidgetExt;

use crate::backend::Client;

use super::window::Window;

gtk::glib::wrapper! {
    pub struct ExportDialog(ObjectSubclass<imp::ExportDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl ExportDialog {
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
    use gtk::prelude::ObjectExt;
    use gtk::subclass::prelude::*;
    use gtk::{CompositeTemplate, FileDialog, FileFilter, Stack};
    use gtk::{glib, template_callbacks};

    use crate::backend::Client;

    #[derive(CompositeTemplate, Default, Properties)]
    #[properties(wrapper_type = super::ExportDialog)]
    #[template(resource = "/ui/export_dialog.ui")]
    pub struct ExportDialog {
        #[template_child]
        stack: TemplateChild<Stack>,
        #[property(get, set)]
        client: RefCell<Client>,
    }

    #[template_callbacks]
    impl ExportDialog {
        #[template_callback]
        fn export_pipeline(&self) {
            let obj = self.obj();
            let window = obj.window();

            let file_dialog = FileDialog::builder()
                .accept_label(gettextrs::gettext("Export"))
                .title(gettextrs::gettext("Export Pipeline"))
                .initial_name("pipeline-export.sqlite")
                .build();
            file_dialog.save(
                Some(&window),
                None::<&Cancellable>,
                clone!(
                    #[weak]
                    obj,
                    #[weak]
                    window,
                    move |res| {
                        if let Ok(file) = res {
                            if let Err(e) = obj.client().backup(file) {
                                window.display_error(
                                    gettextrs::gettext("Error exporting pipeline subscriptions"),
                                    e,
                                );
                            } else {
                                window.show_toast(Toast::new(&gettextrs::gettext(
                                    "Successfully exported data",
                                )));
                            }
                            obj.close();
                        }
                    }
                ),
            )
        }

        #[template_callback]
        fn export_youtube(&self) {
            let obj = self.obj();
            let window = obj.window();

            let csv_filter = FileFilter::new();
            csv_filter.add_mime_type("text/csv");
            let file_dialog = FileDialog::builder()
                .accept_label(gettextrs::gettext("Export"))
                .default_filter(&csv_filter)
                .title(gettextrs::gettext("Export YouTube Subscriptions"))
                .initial_name("pipeline-export-youtube.csv")
                .build();
            file_dialog.save(
                Some(&window),
                None::<&Cancellable>,
                clone!(
                    #[weak]
                    obj,
                    #[weak]
                    window,
                    move |res| {
                        if let Ok(file) = res {
                            if let Err(e) = obj.client().export_youtube(file) {
                                window.display_error(
                                    gettextrs::gettext("Error exporting YouTube subscriptions"),
                                    e,
                                );
                            } else {
                                window.show_toast(Toast::new(&gettextrs::gettext(
                                    "Successfully exported subscriptions",
                                )));
                            }
                            obj.close();
                        }
                    }
                ),
            )
        }

        #[template_callback]
        fn export_newpipe(&self) {
            let obj = self.obj();
            let window = obj.window();

            let json_filter = FileFilter::new();
            json_filter.add_mime_type("application/json");
            json_filter.add_mime_type("application/vnd.document+json");
            let file_dialog = FileDialog::builder()
                .accept_label(gettextrs::gettext("Export"))
                .default_filter(&json_filter)
                .title(gettextrs::gettext("Export NewPipe Subscriptions"))
                .initial_name("pipeline-export-newpipe.json")
                .build();
            file_dialog.save(
                Some(&window),
                None::<&Cancellable>,
                clone!(
                    #[weak]
                    obj,
                    #[weak]
                    window,
                    move |res| {
                        if let Ok(file) = res {
                            if let Err(e) = obj.client().export_newpipe(file) {
                                window.display_error(
                                    gettextrs::gettext("Error exporting NewPipe subscriptions"),
                                    e,
                                );
                            } else {
                                window.show_toast(Toast::new(&gettextrs::gettext(
                                    "Successfully exported subscriptions",
                                )));
                            }
                            obj.close();
                        }
                    }
                ),
            )
        }

        #[template_callback]
        fn export(&self) {
            match self.stack.visible_child_name().as_deref() {
                Some("YouTube") => self.export_youtube(),
                Some("Pipeline") => self.export_pipeline(),
                Some("NewPipe") => self.export_newpipe(),
                _ => log::warn!("Unknown import format; doing nothing"),
            }
        }

        #[template_callback(function)]
        fn get_format(object: &gtk::StringObject) -> glib::GString {
            object.string()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ExportDialog {
        const NAME: &'static str = "TFExportDialog";
        type Type = super::ExportDialog;
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
    impl ObjectImpl for ExportDialog {}
    impl WidgetImpl for ExportDialog {}
    impl AdwDialogImpl for ExportDialog {}
}
