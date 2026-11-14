use std::borrow::Cow;

use gdk::glib::Object;
use glib::{object::Cast, subclass::types::ObjectSubclassIsExt};
use gtk::prelude::WidgetExt;

use crate::{backend::Client, gui::window::Window};

gtk::glib::wrapper! {
    pub struct PreferencesDialog(ObjectSubclass<imp::PreferencesDialog>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

pub fn download_formats() -> [(Cow<'static, str>, &'static str); 5] {
    [
        (
            Cow::Borrowed("480p"),
            "bv*[height<=480]+ba/b[height<=480] / wv*+ba/w",
        ),
        (
            Cow::Borrowed("720p"),
            "bv*[height<=720]+ba/b[height<=720] / wv*+ba/w",
        ),
        (
            Cow::Borrowed("1080p"),
            "bv*[height<=1080]+ba/b[height<=1080] / wv*+ba/w",
        ),
        // Translators: For selecting the best video format when downloading.
        (Cow::Owned(gettextrs::gettext("Best")), "bv*+ba/b"),
        // Translators: For selecting a custom video format when downloading.
        (Cow::Owned(gettextrs::gettext("Custom")), ""),
    ]
}

impl PreferencesDialog {
    pub fn new(client: Client) -> Self {
        let s: Self = Object::builder().property("client", client).build();
        // Requires client to be set up, therefore cannot be done in construction.
        s.imp().init_watch_button_and_switch();
        s
    }

    fn window(&self) -> Window {
        self.root().unwrap().dynamic_cast().unwrap()
    }
}

pub mod imp {
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::rc::Rc;

    use adw::ActionRow;
    use adw::AlertDialog;
    use adw::ButtonRow;
    use adw::ComboRow;
    use adw::EntryRow;
    use adw::PreferencesGroup;
    use adw::SwitchRow;
    use adw::prelude::ActionRowExt;
    use adw::prelude::AlertDialogExt;
    use adw::prelude::AlertDialogExtManual;
    use adw::prelude::ComboRowExt;
    use adw::prelude::EntryRowExt;
    use adw::prelude::PreferencesGroupExt;
    use adw::prelude::PreferencesRowExt;
    use adw::subclass::prelude::AdwDialogImpl;
    use adw::subclass::prelude::PreferencesDialogImpl;
    use gdk::ContentProvider;
    use gdk::DragAction;
    use gdk::gio::Settings;
    use gdk::gio::SettingsBindFlags;
    use gdk::gio::prelude::FileExt;
    use gdk::prelude::DragExt;
    use gdk::prelude::SettingsExt;
    use gdk::prelude::SettingsExtManual;
    use glib::Properties;
    use glib::clone;
    use glib::object::Cast;
    use glib::object::CastNone;
    use glib::object::ObjectExt;
    use glib::subclass::InitializingObject;
    use glib::types::StaticType;
    use glib::value::ToValue;
    use gtk::Align;
    use gtk::Button;
    use gtk::CheckButton;
    use gtk::CompositeTemplate;
    use gtk::DragIcon;
    use gtk::DragSource;
    use gtk::DropControllerMotion;
    use gtk::DropTarget;
    use gtk::FileDialog;
    use gtk::FileFilter;
    use gtk::Image;
    use gtk::InputHints;
    use gtk::ListBox;
    use gtk::ListBoxRow;
    use gtk::StateFlags;
    use gtk::glib;
    use gtk::prelude::ButtonExt;
    use gtk::prelude::CheckButtonExt;
    use gtk::prelude::EditableExt;
    use gtk::prelude::ListBoxRowExt;
    use gtk::prelude::WidgetExt;
    use gtk::subclass::prelude::*;
    use url::Url;

    use crate::backend::Client;
    use crate::gspawn;
    use crate::gui::utility::Utility;

    #[derive(CompositeTemplate, Properties)]
    #[properties(wrapper_type = super::PreferencesDialog)]
    #[template(resource = "/ui/preferences_dialog.ui")]
    pub struct PreferencesDialog {
        #[template_child]
        group_external: TemplateChild<PreferencesGroup>,

        #[template_child]
        switch_external_default: TemplateChild<SwitchRow>,
        #[template_child]
        switch_show_splash: TemplateChild<SwitchRow>,
        #[template_child]
        switch_seasonal_style: TemplateChild<SwitchRow>,
        #[template_child]
        entry_external_player: TemplateChild<EntryRow>,

        #[template_child]
        combo_download_format: TemplateChild<ComboRow>,
        #[template_child]
        entry_custom_download_format: TemplateChild<EntryRow>,
        #[template_child]
        entry_custom_yt_dlp_arguments: TemplateChild<EntryRow>,

        #[template_child]
        switch_reload_on_startup: TemplateChild<SwitchRow>,
        #[template_child]
        switch_watch_history_enabled: TemplateChild<SwitchRow>,
        #[template_child]
        btn_clear_watch_history: TemplateChild<ButtonRow>,
        #[template_child]
        switch_source_youtube: TemplateChild<SwitchRow>,
        #[template_child]
        piped_api_urls: TemplateChild<ListBox>,

        #[template_child]
        row_youtube_cookies: TemplateChild<ActionRow>,
        #[template_child]
        img_youtube_cookies_choose: TemplateChild<Image>,
        #[template_child]
        img_youtube_cookies_remove: TemplateChild<Image>,
        #[template_child]
        entry_youtube_extractor_args: TemplateChild<EntryRow>,

        #[template_child]
        switch_source_peertube: TemplateChild<SwitchRow>,
        #[template_child]
        entry_peertube_search_api: TemplateChild<EntryRow>,

        #[template_child]
        switch_cairo: TemplateChild<SwitchRow>,

        #[template_child]
        check_button_rustypipe_then_piped: TemplateChild<CheckButton>,
        #[template_child]
        check_button_piped_then_rustypipe: TemplateChild<CheckButton>,
        #[template_child]
        check_button_rustypipe: TemplateChild<CheckButton>,
        #[template_child]
        check_button_piped: TemplateChild<CheckButton>,

        #[property(get, set)]
        client: RefCell<Client>,

        settings: Settings,
        yt_dlp_settings: Option<Settings>,
    }

    #[gtk::template_callbacks]
    impl PreferencesDialog {
        fn init_settings(&self) {
            self.settings
                .bind(
                    "reload-on-startup",
                    &self.switch_reload_on_startup.get(),
                    "active",
                )
                .flags(SettingsBindFlags::DEFAULT)
                .build();
            self.settings
                .bind("show-splash", &self.switch_show_splash.get(), "active")
                .flags(SettingsBindFlags::DEFAULT)
                .build();
            self.settings
                .bind(
                    "seasonal-style",
                    &self.switch_seasonal_style.get(),
                    "active",
                )
                .flags(SettingsBindFlags::DEFAULT)
                .build();
            self.settings
                .bind(
                    "watch-history-enabled",
                    &self.switch_watch_history_enabled.get(),
                    "active",
                )
                .flags(SettingsBindFlags::DEFAULT)
                .build();
            self.settings
                .bind(
                    "play-external-default",
                    &self.switch_external_default.get(),
                    "active",
                )
                .flags(SettingsBindFlags::DEFAULT)
                .build();

            self.settings
                .bind(
                    "cairo-software-renderer",
                    &self.switch_cairo.get(),
                    "active",
                )
                .flags(SettingsBindFlags::DEFAULT)
                .build();

            self.entry_peertube_search_api
                .set_text(&self.settings.string("peertube-search-api"));
            self.entry_external_player
                .set_text(&self.settings.string("player"));
            self.settings
                .bind(
                    "enable-source-youtube",
                    &self.switch_source_youtube.get(),
                    "active",
                )
                .build();
            self.settings
                .bind(
                    "enable-source-peertube",
                    &self.switch_source_peertube.get(),
                    "active",
                )
                .build();
            self.settings
                .bind(
                    "enable-source-youtube",
                    &self.switch_source_peertube.get(),
                    "sensitive",
                )
                .build();
            self.settings
                .bind(
                    "enable-source-peertube",
                    &self.switch_source_youtube.get(),
                    "sensitive",
                )
                .build();

            let youtube_mode = self.settings.string("youtube-mode");
            self.check_button_rustypipe
                .set_active(youtube_mode == "rustypipe");
            self.check_button_piped.set_active(youtube_mode == "piped");
            self.check_button_rustypipe_then_piped
                .set_active(youtube_mode == "rustypipe-then-piped");
            self.check_button_piped_then_rustypipe
                .set_active(youtube_mode == "piped-then-rustypipe");
            self.check_button_rustypipe.connect_toggled(clone!(
                #[strong(rename_to=settings)]
                self.settings,
                move |btn| {
                    if btn.is_active() {
                        let _ = settings.set_string("youtube-mode", "rustypipe");
                    }
                }
            ));
            self.check_button_piped.connect_toggled(clone!(
                #[strong(rename_to=settings)]
                self.settings,
                move |btn| {
                    if btn.is_active() {
                        let _ = settings.set_string("youtube-mode", "piped");
                    }
                }
            ));
            self.check_button_rustypipe_then_piped
                .connect_toggled(clone!(
                    #[strong(rename_to=settings)]
                    self.settings,
                    move |btn| {
                        if btn.is_active() {
                            let _ = settings.set_string("youtube-mode", "rustypipe-then-piped");
                        }
                    }
                ));
            self.check_button_piped_then_rustypipe
                .connect_toggled(clone!(
                    #[strong(rename_to=settings)]
                    self.settings,
                    move |btn| {
                        if btn.is_active() {
                            let _ = settings.set_string("youtube-mode", "piped-then-rustypipe");
                        }
                    }
                ));

            if let Some(yt_dlp) = &self.yt_dlp_settings {
                yt_dlp
                    .bind(
                        "extractor-args",
                        &self.entry_youtube_extractor_args.get(),
                        "text",
                    )
                    .build();
            }
        }

        fn create_piped_row(&self, api: Url) -> ActionRow {
            // Inspired by <https://github.com/workbenchdev/demos/blob/f0e3559c0f7cfd8a083bac51d17374f9460d624d/src/Drag%20and%20Drop/main.py>.
            let row = ActionRow::builder().title(api.to_string()).build();

            // Source of drag.
            // TODO: Does not seem to work.
            let xy: Rc<Cell<(f64, f64)>> = Rc::new(Cell::new((0.0, 0.0)));

            let icon = Image::from_icon_name("list-drag-handle-symbolic");
            icon.add_css_class("dim-label");
            row.add_prefix(&icon);

            let remove_button = Button::from_icon_name("user-trash-symbolic");
            remove_button.set_valign(Align::Center);
            remove_button.add_css_class("flat");
            remove_button.connect_clicked(clone!(
                #[weak(rename_to = s)]
                self,
                #[weak(rename_to = list)]
                self.piped_api_urls,
                #[weak]
                row,
                move |_| {
                    list.remove(&row);
                    s.piped_apis_updated();
                }
            ));
            row.add_suffix(&remove_button);

            let drop_controller = DropControllerMotion::new();
            let drag_source = DragSource::builder().actions(DragAction::MOVE).build();

            drag_source.connect_prepare(clone!(
                #[weak]
                row,
                #[weak]
                xy,
                #[upgrade_or_default]
                move |_, x, y| {
                    xy.replace((x, y));
                    Some(ContentProvider::for_value(&row.to_value()))
                }
            ));

            drag_source.connect_drag_begin(clone!(
                #[weak]
                row,
                #[weak]
                xy,
                move |_, drag| {
                    let drag_widget = ListBox::new();
                    drag_widget.set_size_request(row.width(), row.height());
                    drag_widget.add_css_class("boxed-list");

                    let drag_row = ActionRow::builder().title(row.title()).build();
                    let icon = Image::from_icon_name("list-drag-handle-symbolic");
                    icon.add_css_class("dim-label");
                    drag_row.add_prefix(&icon);
                    drag_widget.append(&drag_row);
                    drag_widget.drag_highlight_row(&drag_row);

                    let icon = DragIcon::for_drag(drag);
                    icon.set_child(Some(&drag_widget));

                    let (x, y) = xy.get();
                    drag.set_hotspot(x as i32, y as i32);
                }
            ));

            drop_controller.connect_enter(clone!(
                #[weak]
                row,
                #[weak(rename_to = list)]
                self.piped_api_urls,
                move |_, _, _| {
                    list.drag_highlight_row(&row);
                }
            ));

            drop_controller.connect_leave(clone!(
                #[weak(rename_to = list)]
                self.piped_api_urls,
                move |_| {
                    list.drag_unhighlight_row();
                },
            ));

            row.add_controller(drop_controller);
            row.add_controller(drag_source);

            row
        }

        fn init_piped_preferences_urls(&self) {
            let is_reset = self.piped_api_urls.first_child().is_some();
            let automatic = self
                .settings
                .boolean("piped-automatically-manage-instances");

            if is_reset {
                self.piped_api_urls.remove_all();
            }

            if !is_reset {
                let drop_target = DropTarget::new(ActionRow::static_type(), DragAction::MOVE);

                drop_target.connect_drop(clone!(
                    #[weak(rename_to = s)]
                    self,
                    #[weak(rename_to = list)]
                    self.piped_api_urls,
                    #[upgrade_or_default]
                    move |_, value, _, y| {
                        let Some(target_row) = list.row_at_y(y as i32) else {
                            return false;
                        };
                        if target_row.is::<EntryRow>() {
                            return false;
                        }
                        let target_index = target_row.index();

                        let Ok(row) = value.get::<ListBoxRow>() else {
                            return false;
                        };

                        list.remove(&row);
                        list.insert(&row, target_index);
                        target_row.set_state_flags(StateFlags::NORMAL, true);

                        s.piped_apis_updated();

                        true
                    }
                ));

                self.piped_api_urls.add_controller(drop_target);
            }

            let apis: Vec<String> = self.settings.get("piped-api-urls");

            let automatic_row = SwitchRow::builder()
                .title(gettextrs::gettext("Automatically Manage Piped Instances"))
                .subtitle(gettextrs::gettext("Recommended"))
                .build();
            self.piped_api_urls.append(&automatic_row);
            self.settings
                .bind(
                    "piped-automatically-manage-instances",
                    &automatic_row,
                    "active",
                )
                .flags(SettingsBindFlags::DEFAULT)
                .build();

            for api in apis {
                if let Ok(url) = Url::parse(&api) {
                    let row = self.create_piped_row(url);
                    row.set_visible(!automatic);
                    self.piped_api_urls.append(&row);
                }
            }

            let create_new_row = EntryRow::builder()
                .title(gettextrs::gettext("New Piped API"))
                .show_apply_button(true)
                .build();
            create_new_row.set_visible(!automatic);

            automatic_row.connect_active_notify(clone!(
                #[weak(rename_to = s)]
                self,
                move |_| {
                    s.piped_automatic_changed();
                }
            ));

            create_new_row.connect_apply(clone!(
                #[weak(rename_to = s)]
                self,
                move |create_new_row| {
                    let text = create_new_row.text();
                    if let Ok(url) = Url::parse(&text) {
                        let row = s.create_piped_row(url);

                        s.piped_api_urls.remove(create_new_row);
                        s.piped_api_urls.append(&row);
                        s.piped_api_urls.append(create_new_row);
                        create_new_row.remove_css_class("error");
                        create_new_row.set_text("");

                        s.piped_apis_updated();
                    } else {
                        create_new_row.add_css_class("error");
                    }
                }
            ));
            create_new_row.set_input_purpose(gtk::InputPurpose::Url);
            create_new_row.set_input_hints(
                InputHints::NO_SPELLCHECK & InputHints::LOWERCASE & InputHints::NO_EMOJI,
            );

            self.piped_api_urls.append(&create_new_row);

            if !is_reset {
                self.piped_apis_updated();
                self.piped_automatic_changed();
            }
        }

        fn prompt_youtube_cookies(&self) {
            let obj = self.obj();
            if let Some(yt_dlp) = &self.yt_dlp_settings {
                let confirmation_dialog = AlertDialog::builder()
                    .body(gettextrs::gettext("If watching too many YouTube videos, YouTube can require confirmation that you are a real person. This is done by logging into your account using cookies stored in your browser. Refer to the <a href=\"https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp\">yt-dlp documentation</a> on how to obtain such a file. Note that this associates the videos you watch with your account. Furthermore, this may get your account deactivated due to not using offical YouTube applications. The developers of Pipeline are not responsible for your account being deactivated."))
                    .body_use_markup(true)
                    .prefer_wide_layout(true)
                    .build();
                confirmation_dialog.add_response(
                    "set-cookies",
                    &gettextrs::gettext("Accept the Risk and Continue"),
                );
                confirmation_dialog
                    .set_response_appearance("set-cookies", adw::ResponseAppearance::Destructive);
                confirmation_dialog.add_response("cancel", &gettextrs::gettext("Cancel"));
                confirmation_dialog.set_default_response(Some("cancel"));

                confirmation_dialog.choose(
                    Some(&obj.window()),
                    None::<&gdk::gio::Cancellable>,
                    clone!(
                        #[strong]
                        obj,
                        #[strong]
                        yt_dlp,
                        move |response| {
                            if response == "set-cookies" {
                                let plain_filter = FileFilter::new();
                                plain_filter.add_mime_type("text/plain");

                                let dialog = FileDialog::builder()
                                    .accept_label(gettextrs::gettext("Select File"))
                                    .title(gettextrs::gettext("Select Cookies File"))
                                    .default_filter(&plain_filter)
                                    .build();
                                dialog.open(
                                    Some(&obj.window()),
                                    None::<&gdk::gio::Cancellable>,
                                    clone!(
                                        #[strong]
                                        obj,
                                        #[strong]
                                        yt_dlp,
                                        move |res| if let Ok(file) = res {
                                            let _ = yt_dlp.set_string(
                                                "cookies-file",
                                                &file
                                                    .path()
                                                    .as_ref()
                                                    .map(|p| p.to_string_lossy())
                                                    .unwrap_or_default(),
                                            );
                                            obj.imp().youtube_cookies_changed();
                                        }
                                    ),
                                );
                            }
                        }
                    ),
                );
            }
        }

        fn init_youtube_cookies(&self) {
            self.youtube_cookies_changed();

            if let Some(yt_dlp) = &self.yt_dlp_settings {
                let obj = self.obj();
                self.row_youtube_cookies.connect_activated(clone!(
                    #[strong]
                    obj,
                    #[strong]
                    yt_dlp,
                    move |_| {
                        let current_file = yt_dlp.string("cookies-file");

                        if current_file.is_empty() {
                            obj.imp().prompt_youtube_cookies();
                        } else {
                            let _ = yt_dlp.set_string("cookies-file", "");
                            obj.imp().youtube_cookies_changed();
                        }
                    }
                ));
            }
        }

        fn youtube_cookies_changed(&self) {
            if let Some(yt_dlp) = &self.yt_dlp_settings {
                let file = yt_dlp.string("cookies-file");

                if file.is_empty() {
                    self.img_youtube_cookies_choose.set_visible(true);
                    self.img_youtube_cookies_remove.set_visible(false);
                    self.row_youtube_cookies
                        .set_activatable_widget(Some(&self.img_youtube_cookies_choose.get()));

                    self.row_youtube_cookies
                        .set_subtitle("No cookies file used");
                } else {
                    self.img_youtube_cookies_choose.set_visible(false);
                    self.img_youtube_cookies_remove.set_visible(true);
                    self.row_youtube_cookies
                        .set_activatable_widget(Some(&self.img_youtube_cookies_remove.get()));

                    self.row_youtube_cookies.set_subtitle(&file);
                }
            }
        }

        fn piped_automatic_changed(&self) {
            let automatic = self
                .settings
                .boolean("piped-automatically-manage-instances");

            // Skip one child, that is the "automatic" switch.
            let mut child = self.piped_api_urls.first_child().unwrap().next_sibling();
            while let Some(row) = child {
                row.set_visible(!automatic);
                child = row.next_sibling();
            }

            if automatic {
                let obj = self.obj();
                let client = self.obj().client();
                gspawn!(clone!(
                    #[weak]
                    client,
                    #[weak]
                    obj,
                    async move {
                        client.setup_async().await;
                        obj.imp().init_piped_preferences_urls();
                    }
                ));
            }
        }

        fn piped_apis_updated(&self) {
            let mut urls = vec![];

            let mut n_action_rows = 0;
            // Skip one child, that is the "automatic" switch.
            let mut child = self.piped_api_urls.first_child().unwrap().next_sibling();
            while let Some(row) = child {
                if let Some(action_row) = row.dynamic_cast_ref::<ActionRow>() {
                    n_action_rows += 1;
                    action_row.set_sensitive(true);
                    urls.push(action_row.title().to_string());
                }

                child = row.next_sibling();
            }

            if n_action_rows == 1 {
                // Skip one child, that is the "automatic" switch.
                if let Ok(action_row) = self
                    .piped_api_urls
                    .first_child()
                    .unwrap()
                    .next_sibling()
                    .and_dynamic_cast::<ActionRow>()
                {
                    action_row.set_sensitive(false);
                }
            }

            let _ = self.settings.set("piped-api-urls", urls);
        }

        #[template_callback]
        fn apply_peertube_search_api(&self) {
            let text = self.entry_peertube_search_api.text();

            if Url::parse(&text).is_ok() {
                self.entry_peertube_search_api.remove_css_class("error");

                let _ = self.settings.set_string("peertube-search-api", &text);
            } else {
                self.entry_peertube_search_api.add_css_class("error");
            }
        }

        pub(super) fn init_watch_button_and_switch(&self) {
            let client = self.client.borrow();
            let obj = self.obj();
            self.btn_clear_watch_history
                .set_sensitive(client.has_watch_history());

            self.switch_watch_history_enabled
                .connect_active_notify(clone!(
                    #[weak]
                    obj,
                    move |row| {
                        if !row.is_active() {
                            obj.imp().clear_watch_history();
                        }
                    }
                ));
        }

        #[template_callback]
        fn apply_external_player(&self) {
            let text = self.entry_external_player.text();

            let _ = self.settings.set_string("player", &text);
        }

        #[template_callback]
        fn clear_watch_history(&self) {
            self.client.borrow().delete_watch_history();
            self.btn_clear_watch_history.set_sensitive(false);
        }

        fn init_download(&self) {
            let obj = self.obj();

            // Custom yt-dlp arguemts.
            self.settings
                .bind(
                    "download-yt-dlp-arguments",
                    &self.entry_custom_yt_dlp_arguments.get(),
                    "text",
                )
                .build();

            // Download format.
            let download_formats = super::download_formats();
            // Assumtion: "Custom" is the last download format.
            let custom_format_index = download_formats.len() - 1;
            let format_model = gtk::StringList::new(
                &download_formats
                    .iter()
                    .map(|f| &f.0)
                    .map(std::borrow::Borrow::borrow)
                    .collect::<Vec<_>>(),
            );

            let current_format = self.settings.string("download-format");
            let current_format_index = download_formats
                .iter()
                .position(|f| f.1 == current_format)
                .unwrap_or(custom_format_index);

            self.combo_download_format.set_model(Some(&format_model));
            self.combo_download_format.connect_selected_notify(clone!(
                #[weak]
                obj,
                move |r| {
                    let i = r.selected() as usize;
                    let format = download_formats[i].1;

                    if !format.is_empty() {
                        obj.imp()
                            .settings
                            .set("download-format", format)
                            .expect("Failed to set download-format setting");
                    }

                    obj.imp()
                        .entry_custom_download_format
                        .set_visible(format.is_empty());
                }
            ));

            self.settings
                .bind(
                    "download-format",
                    &self.entry_custom_download_format.get(),
                    "text",
                )
                .build();

            self.combo_download_format
                .set_selected(current_format_index as u32);
            obj.imp()
                .entry_custom_download_format
                .set_visible(current_format_index == custom_format_index);
        }

        fn init_flatpak(&self) {
            self.group_external.set_description(Some(&gettextrs::gettext("To use external players with Pipeline, you will need to grant Pipeline additional permissions. See <a href=\"https://gitlab.com/schmiddi-on-mobile/pipeline/-/wikis/External-Player-for-Flatpak\">this page</a> for details.")));
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreferencesDialog {
        const NAME: &'static str = "TFPreferencesDialog";
        type Type = super::PreferencesDialog;
        type ParentType = adw::PreferencesDialog;

        fn new() -> Self {
            Self {
                group_external: Default::default(),
                switch_external_default: Default::default(),
                switch_show_splash: Default::default(),
                switch_seasonal_style: Default::default(),
                entry_external_player: Default::default(),
                entry_custom_yt_dlp_arguments: Default::default(),
                combo_download_format: Default::default(),
                entry_custom_download_format: Default::default(),
                switch_reload_on_startup: Default::default(),
                switch_watch_history_enabled: Default::default(),
                switch_cairo: Default::default(),
                btn_clear_watch_history: Default::default(),
                switch_source_youtube: Default::default(),
                piped_api_urls: Default::default(),
                row_youtube_cookies: Default::default(),
                img_youtube_cookies_choose: Default::default(),
                img_youtube_cookies_remove: Default::default(),
                entry_youtube_extractor_args: Default::default(),
                switch_source_peertube: Default::default(),
                entry_peertube_search_api: Default::default(),
                check_button_rustypipe: Default::default(),
                check_button_piped: Default::default(),
                check_button_rustypipe_then_piped: Default::default(),
                check_button_piped_then_rustypipe: Default::default(),
                client: Default::default(),
                settings: Settings::new(crate::config::BASE_ID),
                yt_dlp_settings: clapper::global_enhancer_proxies()
                    .and_then(|p| p.proxy_by_module("clapper_yt_dlp"))
                    .and_then(|p| p.settings()),
            }
        }

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
    impl ObjectImpl for PreferencesDialog {
        fn constructed(&self) {
            self.parent_constructed();
            self.init_settings();
            self.init_piped_preferences_urls();
            self.init_youtube_cookies();
            self.init_download();
            if crate::config::FLATPAK {
                self.init_flatpak();
            }
        }
    }
    impl WidgetImpl for PreferencesDialog {}
    impl PreferencesDialogImpl for PreferencesDialog {}
    impl AdwDialogImpl for PreferencesDialog {}
}
