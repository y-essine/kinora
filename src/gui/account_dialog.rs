//! Kinora: connect a YouTube account by picking a browser profile that is
//! signed in to YouTube (or an exported cookies.txt). The choice is tested before
//! it is saved, and saving it makes the Home page load.

use glib::Object;

use crate::backend::Client;

glib::wrapper! {
    pub struct AccountDialog(ObjectSubclass<imp::AccountDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl AccountDialog {
    pub fn new(client: &Client) -> Self {
        Object::builder::<Self>().property("client", client).build()
    }
}

pub mod imp {
    use std::cell::RefCell;

    use adw::prelude::*;
    use adw::subclass::dialog::AdwDialogImpl;
    use gdk::gio::Settings;
    use glib::Properties;
    use glib::clone;
    use glib::subclass::InitializingObject;
    use gtk::CompositeTemplate;
    use gtk::subclass::prelude::*;

    use crate::backend::Client;
    use crate::backend::account::{self, BrowserProfile};
    use crate::config::BASE_ID;
    use crate::{gspawn, tspawn};

    #[derive(CompositeTemplate, Properties)]
    #[template(resource = "/ui/account_dialog.ui")]
    #[properties(wrapper_type = super::AccountDialog)]
    pub struct AccountDialog {
        #[template_child]
        toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        stack: TemplateChild<gtk::Stack>,
        #[template_child]
        group_profiles: TemplateChild<adw::PreferencesGroup>,

        #[property(get, set)]
        client: RefCell<Client>,

        rows: RefCell<Vec<adw::ActionRow>>,
        settings: Settings,
    }

    impl Default for AccountDialog {
        fn default() -> Self {
            Self {
                toast_overlay: Default::default(),
                stack: Default::default(),
                group_profiles: Default::default(),
                client: Default::default(),
                rows: Default::default(),
                settings: Settings::new(BASE_ID),
            }
        }
    }

    #[gtk::template_callbacks]
    impl AccountDialog {
        #[template_callback]
        fn scan(&self) {
            self.stack.set_visible_child_name("scanning");
            let obj = self.obj();
            gspawn!(clone!(
                #[weak]
                obj,
                async move {
                    let result = tspawn!(async { account::browser_profiles().await })
                        .await
                        .expect("Failed to spawn tokio");
                    obj.imp().show_profiles(result);
                }
            ));
        }

        #[template_callback]
        fn choose_file(&self) {
            let obj = self.obj();
            let filter = gtk::FileFilter::new();
            filter.add_pattern("*.txt");
            filter.set_name(Some(&gettextrs::gettext("Cookies files")));
            let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder()
                .title(gettextrs::gettext("Choose a cookies.txt File"))
                .filters(&filters)
                .modal(true)
                .build();
            let window = obj.root().and_downcast::<gtk::Window>();
            dialog.open(
                window.as_ref(),
                None::<&gtk::gio::Cancellable>,
                clone!(
                    #[weak]
                    obj,
                    move |result| {
                        if let Some(path) = result.ok().and_then(|f| f.path()) {
                            obj.imp().connect(
                                account::file_command(&path),
                                gettextrs::gettext("cookies.txt file"),
                            );
                        }
                    }
                ),
            );
        }
    }

    impl AccountDialog {
        fn show_profiles(&self, result: Result<Vec<BrowserProfile>, account::Error>) {
            for row in self.rows.take() {
                self.group_profiles.remove(&row);
            }
            let profiles = match result {
                Ok(profiles) => profiles,
                Err(e) => {
                    self.stack.set_visible_child_name("empty");
                    self.toast(&e.to_string());
                    return;
                }
            };
            if profiles.is_empty() {
                self.stack.set_visible_child_name("empty");
                return;
            }

            let connected = self.settings.string("account-cookies-command").to_string();
            let mut rows = Vec::new();
            for profile in profiles {
                let is_current = profile.command() == connected;
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&profile.name))
                    .subtitle(glib::markup_escape_text(&format!(
                        "{} · {}",
                        profile.browser_name,
                        std::path::Path::new(&profile.profile)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| profile.profile.clone())
                    )))
                    .activatable(true)
                    .build();
                row.add_prefix(&gtk::Image::from_icon_name("avatar-default-symbolic"));
                row.add_suffix(&gtk::Image::from_icon_name(if is_current {
                    "object-select-symbolic"
                } else {
                    "go-next-symbolic"
                }));
                row.connect_activated(clone!(
                    #[weak(rename_to = s)]
                    self,
                    move |_| s.connect(profile.command(), profile.label())
                ));
                self.group_profiles.add(&row);
                rows.push(row);
            }
            self.rows.replace(rows);
            self.stack.set_visible_child_name("list");
        }

        /// Test a cookies command; save it (which loads Home) only if it works.
        pub(super) fn connect(&self, command: String, label: String) {
            self.stack.set_visible_child_name("connecting");
            let obj = self.obj();
            gspawn!(clone!(
                #[weak]
                obj,
                async move {
                    let cmd = command.clone();
                    let result = tspawn!(async move { account::test_command(&cmd).await })
                        .await
                        .expect("Failed to spawn tokio");
                    let imp = obj.imp();
                    match result {
                        Ok(()) => {
                            let _ = imp.settings.set_string("account-label", &label);
                            let _ = imp.settings.set_string("account-cookies-command", &command);
                            obj.close();
                        }
                        Err(e) => {
                            imp.stack
                                .set_visible_child_name(if imp.rows.borrow().is_empty() {
                                    "empty"
                                } else {
                                    "list"
                                });
                            imp.toast(&e.to_string());
                        }
                    }
                }
            ));
        }

        fn toast(&self, message: &str) {
            self.toast_overlay.add_toast(
                adw::Toast::builder()
                    .title(glib::markup_escape_text(message))
                    .timeout(6)
                    .build(),
            );
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AccountDialog {
        const NAME: &'static str = "TFAccountDialog";
        type Type = super::AccountDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for AccountDialog {
        fn constructed(&self) {
            self.parent_constructed();
            self.scan();
        }
    }

    impl WidgetImpl for AccountDialog {}
    impl AdwDialogImpl for AccountDialog {}
}
