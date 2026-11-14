//! Kinora: the Extensions page of the preferences. One group per extension
//! of `client.core().extensions()`: a switch in the header, what it is allowed
//! to do, and a row per setting it declares.
//!
//! Everything is persisted in GSettings, which the [`Client`] hands on to the
//! core and reacts to right away (no restart): `disabled-extensions` holds the
//! ids that are off, `extension-settings` the values (see
//! `backend::extension_values`).

use adw::prelude::*;
use gdk::gio::Settings;
use gdk::prelude::{SettingsExt, SettingsExtManual};
use gtk::glib;
use kinora_core::extension::{
    Permission, Setting, SettingKind, SettingValue, SettingsPanel, value_key,
};

use crate::backend::Client;
use crate::backend::extension_values::with_value;

const DISABLED: &str = "disabled-extensions";
const VALUES: &str = "extension-settings";

/// Add a group for every registered extension to `page`.
pub fn populate(page: &adw::PreferencesPage, client: &Client, settings: &Settings) {
    for panel in client.core().extensions().settings_panels() {
        page.add(&extension_group(&panel, client, settings));
    }
}

/// What a permission means to the user, in plain words.
fn permission_label(permission: Permission) -> String {
    match permission {
        Permission::Network => gettextrs::gettext("Uses the network"),
        Permission::Account => gettextrs::gettext("Reads your account"),
        Permission::PlayerControl => gettextrs::gettext("Controls the player"),
    }
}

/// "Uses the network · Reads your account", or a note when it needs nothing.
fn permissions_summary(permissions: &[Permission]) -> String {
    if permissions.is_empty() {
        return gettextrs::gettext("Needs no special access");
    }
    permissions
        .iter()
        .map(|p| permission_label(*p))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn extension_group(
    panel: &SettingsPanel,
    client: &Client,
    settings: &Settings,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(&glib::markup_escape_text(&gettextrs::gettext(&panel.title)));
    group.set_description(Some(&glib::markup_escape_text(&gettextrs::gettext(
        &panel.description,
    ))));

    let switch = gtk::Switch::builder()
        .active(panel.enabled)
        .valign(gtk::Align::Center)
        .build();
    switch.update_property(&[gtk::accessible::Property::Label(
        // Translators: {} is the name of an extension.
        &gettextrs::gettext("Enable {}").replace("{}", &panel.title),
    )]);
    group.set_header_suffix(Some(&switch));
    switch.connect_active_notify({
        let id = panel.extension.clone();
        let client = client.clone();
        let settings = settings.clone();
        move |switch| {
            // The registry computes the new list (and keeps ids of extensions
            // that are not installed); the Client reloads it from GSettings.
            let disabled = client
                .core()
                .extensions()
                .set_enabled(&id, switch.is_active());
            if let Err(e) = settings.set_strv(DISABLED, disabled) {
                log::warn!("Could not store {DISABLED}: {e}");
            }
        }
    });

    let access = adw::ActionRow::builder()
        .title(gettextrs::gettext("Permissions"))
        .subtitle(glib::markup_escape_text(&permissions_summary(
            &panel.permissions,
        )))
        .build();
    group.add(&access);

    for setting in &panel.settings {
        let value = panel
            .values
            .get(&setting.key)
            .cloned()
            .unwrap_or_else(|| setting.kind.default_value());
        let row = setting_row(&panel.extension, setting, &value, settings);
        // A switched-off extension's settings can't be changed.
        switch
            .bind_property("active", &row, "sensitive")
            .sync_create()
            .build();
        group.add(&row);
    }
    group
}

/// Store one setting value in `extension-settings`.
fn store(settings: &Settings, extension: &str, key: &str, value: SettingValue) {
    let dict = with_value(&settings.value(VALUES), &value_key(extension, key), &value);
    if let Err(e) = settings.set_value(VALUES, &dict) {
        log::warn!("Could not store {VALUES}: {e}");
    }
}

/// The row that shows and changes one setting: a switch for a toggle, a combo
/// row for a choice, an entry for text.
fn setting_row(
    extension: &str,
    setting: &Setting,
    value: &SettingValue,
    settings: &Settings,
) -> gtk::Widget {
    let title = glib::markup_escape_text(&gettextrs::gettext(&setting.title));
    let subtitle = setting
        .subtitle
        .as_deref()
        .map(|s| glib::markup_escape_text(&gettextrs::gettext(s)));
    let extension = extension.to_owned();
    let key = setting.key.clone();
    let settings = settings.clone();

    match &setting.kind {
        SettingKind::Toggle { .. } => {
            let row = adw::SwitchRow::builder()
                .title(title)
                .active(matches!(value, SettingValue::Bool(true)))
                .build();
            if let Some(subtitle) = subtitle {
                row.set_subtitle(&subtitle);
            }
            row.connect_active_notify(move |row| {
                store(
                    &settings,
                    &extension,
                    &key,
                    SettingValue::Bool(row.is_active()),
                );
            });
            row.upcast()
        }
        SettingKind::Choice { options, .. } => {
            let labels: Vec<String> = options
                .iter()
                .map(|o| gettextrs::gettext(&o.label))
                .collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            let selected = match value {
                SettingValue::Text(v) => options.iter().position(|o| &o.value == v),
                SettingValue::Bool(_) => None,
            };
            let row = adw::ComboRow::builder()
                .title(title)
                .model(&gtk::StringList::new(&labels))
                .selected(selected.map_or(0, |i| i as u32))
                .build();
            if let Some(subtitle) = subtitle {
                row.set_subtitle(&subtitle);
            }
            let values: Vec<String> = options.iter().map(|o| o.value.clone()).collect();
            row.connect_selected_notify(move |row| {
                if let Some(value) = values.get(row.selected() as usize) {
                    store(
                        &settings,
                        &extension,
                        &key,
                        SettingValue::Text(value.clone()),
                    );
                }
            });
            row.upcast()
        }
        SettingKind::Text { .. } => {
            let row = adw::EntryRow::builder()
                .title(title)
                .show_apply_button(true)
                .build();
            if let SettingValue::Text(text) = value {
                row.set_text(text);
            }
            row.connect_apply(move |row| {
                store(
                    &settings,
                    &extension,
                    &key,
                    SettingValue::Text(row.text().into()),
                );
            });
            row.upcast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_are_described_in_plain_words() {
        assert_eq!(
            permissions_summary(&[Permission::Network, Permission::Account]),
            "Uses the network · Reads your account"
        );
        assert_eq!(
            permissions_summary(&[Permission::PlayerControl]),
            "Controls the player"
        );
        assert_eq!(permissions_summary(&[]), "Needs no special access");
    }
}
