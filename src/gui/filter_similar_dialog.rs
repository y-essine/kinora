use adw::prelude::AlertDialogExt;
use gdk::glib::Object;
use glib::subclass::types::ObjectSubclassIsExt;
use gtk::prelude::EditableExt;

use crate::backend::{Client, Filter, Video};

gtk::glib::wrapper! {
    pub struct FilterSimilarDialog(ObjectSubclass<imp::FilterSimilarDialog>)
        @extends adw::AlertDialog, adw::Dialog, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl FilterSimilarDialog {
    pub fn new(client: Client) -> Self {
        Object::builder().property("client", client).build()
    }

    pub fn with_populated_fields_from_video(client: Client, video: Video) -> Self {
        let s = Self::new(client);
        let imp = s.imp();
        imp.entry_title.set_text(&video.title());
        imp.entry_channel.set_text(&video.uploader().name());
        s
    }

    pub fn for_editing(client: Client, filter: Filter) -> Self {
        let s = Self::new(client);
        s.set_editing(Some(&filter));
        let imp = s.imp();
        imp.entry_title.set_text(&filter.title());
        imp.entry_channel.set_text(&filter.channel());
        s.set_heading(Some(&gettextrs::gettext("Edit Filter")));
        s.set_response_label("add-filter", &gettextrs::gettext("Save Filter"));
        s
    }

    pub fn construct_filter(&self) -> Filter {
        let imp = self.imp();
        Filter::new("".into(), imp.entry_title.text(), imp.entry_channel.text())
    }
}

pub mod imp {
    use std::cell::RefCell;

    use adw::EntryRow;
    use adw::subclass::prelude::AdwAlertDialogImpl;
    use adw::subclass::prelude::AdwDialogImpl;
    use glib::Properties;
    use glib::subclass::InitializingObject;
    use gtk::CompositeTemplate;
    use gtk::glib;
    use gtk::prelude::ObjectExt;
    use gtk::subclass::prelude::*;

    use crate::backend::Client;
    use crate::backend::Filter;

    #[derive(CompositeTemplate, Default, Properties)]
    #[properties(wrapper_type = super::FilterSimilarDialog)]
    #[template(resource = "/ui/filter_similar_dialog.ui")]
    pub struct FilterSimilarDialog {
        #[template_child]
        pub(super) entry_title: TemplateChild<EntryRow>,
        #[template_child]
        pub(super) entry_channel: TemplateChild<EntryRow>,

        #[property(get, set)]
        client: RefCell<Client>,

        #[property(get, set, nullable)]
        editing: RefCell<Option<Filter>>,
    }

    #[gtk::template_callbacks]
    impl FilterSimilarDialog {}

    #[glib::object_subclass]
    impl ObjectSubclass for FilterSimilarDialog {
        const NAME: &'static str = "TFFilterSimilarDialog";
        type Type = super::FilterSimilarDialog;
        type ParentType = adw::AlertDialog;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for FilterSimilarDialog {}
    impl WidgetImpl for FilterSimilarDialog {}
    impl AdwAlertDialogImpl for FilterSimilarDialog {
        fn response(&self, response: &str) {
            let obj = self.obj();
            if response == "add-filter" {
                let client = obj.client();
                let filter = obj.construct_filter();
                let has_filter = client.filter_store().has_filter(&filter);
                if has_filter {
                    // TODO: Some error toast?
                    return;
                }
                if let Some(editing) = obj.editing() {
                    client.toggle_filter(editing);
                }
                client.toggle_filter(filter);
            }
        }
    }
    impl AdwDialogImpl for FilterSimilarDialog {}
}
