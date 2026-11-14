use gdk::glib::Object;

use crate::backend::Client;

gtk::glib::wrapper! {
    pub struct FilterDialog(ObjectSubclass<imp::FilterDialog>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl FilterDialog {
    pub fn new(client: Client) -> Self {
        let s: Self = Object::builder().property("client", client).build();
        s
    }
}

pub mod imp {
    use std::cell::Cell;
    use std::cell::RefCell;

    use adw::ActionRow;
    use adw::SwitchRow;
    use adw::prelude::ActionRowExt;
    use adw::prelude::AdwDialogExt;
    use adw::prelude::PreferencesDialogExt;
    use adw::prelude::PreferencesRowExt;
    use adw::subclass::prelude::AdwDialogImpl;
    use adw::subclass::prelude::PreferencesDialogImpl;
    use gdk::gio::Settings;
    use gdk::gio::SettingsBindFlags;
    use gdk::gio::prelude::SettingsExt;
    use gdk::prelude::ListModelExt;
    use gdk::prelude::SettingsExtManual;
    use glib::Properties;
    use glib::clone;
    use glib::object::Cast;
    use glib::subclass::InitializingObject;
    use gtk::Align;
    use gtk::Button;
    use gtk::CompositeTemplate;
    use gtk::ListBox;
    use gtk::glib;
    use gtk::prelude::ButtonExt;
    use gtk::prelude::ObjectExt;
    use gtk::prelude::WidgetExt;
    use gtk::subclass::prelude::*;

    use crate::backend::Client;
    use crate::gui::filter_similar_dialog::FilterSimilarDialog;

    #[derive(CompositeTemplate, Properties)]
    #[properties(wrapper_type = super::FilterDialog)]
    #[template(resource = "/ui/filter_dialog.ui")]
    pub struct FilterDialog {
        #[template_child]
        switch_hide_watched: TemplateChild<SwitchRow>,
        #[template_child]
        switch_hide_shorts: TemplateChild<SwitchRow>,
        #[template_child]
        switch_hide_today: TemplateChild<SwitchRow>,
        #[template_child]
        switch_hide_requires_payment: TemplateChild<SwitchRow>,

        #[template_child]
        custom_filter_box: TemplateChild<ListBox>,

        #[property(get, set)]
        custom_filters_initialized: Cell<bool>,

        settings: Settings,

        #[property(get, set)]
        client: RefCell<Client>,
    }

    #[gtk::template_callbacks]
    impl FilterDialog {
        fn init_settings(&self) {
            if self.settings.boolean("watch-history-enabled") {
                self.settings
                    .bind("remove-watched", &self.switch_hide_watched.get(), "active")
                    .flags(SettingsBindFlags::DEFAULT)
                    .build();
            } else {
                self.switch_hide_watched.set_active(false);
                self.switch_hide_watched.set_sensitive(false);
                self.switch_hide_watched.set_subtitle(&gettextrs::gettext("This filter is disabled due to your preference to not save your watch history. Enable the preference to allow for enabling this filter."));
            }
            self.settings
                .bind(
                    "remove-short-videos",
                    &self.switch_hide_shorts.get(),
                    "active",
                )
                .flags(SettingsBindFlags::DEFAULT)
                .build();
            self.settings
                .bind(
                    "remove-requires-payment",
                    &self.switch_hide_requires_payment.get(),
                    "active",
                )
                .flags(SettingsBindFlags::DEFAULT)
                .build();
            self.settings
                .bind(
                    "only-videos-yesterday",
                    &self.switch_hide_today.get(),
                    "active",
                )
                .flags(SettingsBindFlags::DEFAULT)
                .build();
        }

        pub(super) fn init_custom_filters(&self) {
            let obj = self.obj();
            let client = obj.client();
            let filters = client.filter_store();

            filters.connect_items_changed(clone!(
                #[weak(rename_to = list)]
                self.custom_filter_box,
                move |model, _, _, _| {
                    list.set_visible(model.n_items() != 0);
                }
            ));

            self.custom_filter_box.bind_model(
                Some(&filters),
                clone!(
                    #[weak]
                    obj,
                    #[weak]
                    client,
                    #[upgrade_or_panic]
                    move |filter| {
                        let row = ActionRow::new();
                        row.set_use_markup(false);
                        filter
                            .bind_property("title", &row, "subtitle")
                            .sync_create()
                            .build();
                        filter
                            .bind_property("channel", &row, "title")
                            .sync_create()
                            .build();
                        row.add_css_class("property");

                        let edit_button = Button::from_icon_name("document-edit-symbolic");
                        edit_button.set_valign(Align::Center);
                        edit_button.add_css_class("flat");
                        edit_button.set_tooltip_text(Some(&gettextrs::gettext("Edit Filter")));
                        edit_button.connect_clicked(clone!(
                            #[weak]
                            obj,
                            #[weak]
                            filter,
                            #[weak]
                            client,
                            move |_| {
                                FilterSimilarDialog::for_editing(
                                    client,
                                    filter
                                        .dynamic_cast()
                                        .expect("FilterStore to only contain Filters"),
                                )
                                .present(Some::<
                                    &super::FilterDialog,
                                >(
                                    obj.as_ref()
                                ));
                            }
                        ));
                        row.add_suffix(&edit_button);

                        let remove_button = Button::from_icon_name("user-trash-symbolic");
                        remove_button.set_valign(Align::Center);
                        remove_button.add_css_class("flat");
                        remove_button.set_tooltip_text(Some(&gettextrs::gettext("Remove Filter")));
                        remove_button.connect_clicked(clone!(
                            #[weak]
                            filter,
                            #[weak]
                            client,
                            move |_| {
                                client.toggle_filter(
                                    filter
                                        .dynamic_cast()
                                        .expect("`FilterStore` to only contain `Filter`s"),
                                );
                            }
                        ));
                        row.add_suffix(&remove_button);

                        row.into()
                    }
                ),
            )
        }

        #[template_callback]
        fn add_filter(&self) {
            let dialog = FilterSimilarDialog::new(self.obj().client());
            dialog.present(Some(self.obj().as_ref()));
        }

        fn setup_custom_filter_initialize(&self) {
            let obj = self.obj();
            obj.connect_visible_page_name_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    let name = obj.visible_page_name();
                    if name.as_deref() == Some("custom") && !obj.custom_filters_initialized() {
                        obj.imp().init_custom_filters();
                        obj.set_custom_filters_initialized(true);
                    }
                }
            ));
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FilterDialog {
        const NAME: &'static str = "TFFilterDialog";
        type Type = super::FilterDialog;
        type ParentType = adw::PreferencesDialog;

        fn new() -> Self {
            Self {
                switch_hide_watched: Default::default(),
                switch_hide_shorts: Default::default(),
                switch_hide_today: Default::default(),
                switch_hide_requires_payment: Default::default(),
                custom_filter_box: Default::default(),
                custom_filters_initialized: Default::default(),
                settings: Settings::new(crate::config::BASE_ID),
                client: Default::default(),
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

    #[glib::derived_properties]
    impl ObjectImpl for FilterDialog {
        fn constructed(&self) {
            self.parent_constructed();
            self.init_settings();
            self.setup_custom_filter_initialize();
        }
    }
    impl WidgetImpl for FilterDialog {}
    impl PreferencesDialogImpl for FilterDialog {}
    impl AdwDialogImpl for FilterDialog {}
}
