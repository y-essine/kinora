//! Kinora: the `extension-settings` GSettings key (`a{sv}`) and the core's
//! [`SettingValue`]s. Toggles are stored as booleans, choices and text as
//! strings, under `"EXTENSION.SETTING"` (see [`kinora_core::extension::value_key`]).

use std::collections::HashMap;

use glib::prelude::ToVariant;
use kinora_core::extension::SettingValue;

/// The `extension-settings` dictionary (`a{sv}`) as the core's setting
/// values. Other value types are skipped.
pub fn extension_values(dict: &glib::Variant) -> HashMap<String, SettingValue> {
    let mut values = HashMap::new();
    for (key, value) in entries(dict) {
        let value = if let Some(b) = value.get::<bool>() {
            SettingValue::Bool(b)
        } else if let Some(s) = value.str() {
            SettingValue::Text(s.to_owned())
        } else {
            continue;
        };
        values.insert(key, value);
    }
    values
}

/// A setting value as stored in the dictionary (an unboxed `b` or `s`).
pub fn value_to_variant(value: &SettingValue) -> glib::Variant {
    match value {
        SettingValue::Bool(b) => b.to_variant(),
        SettingValue::Text(t) => t.to_variant(),
    }
}

/// The dictionary with `key` set to `value`, to write back to the setting.
/// Every other entry is kept, also ones this version does not understand.
pub fn with_value(dict: &glib::Variant, key: &str, value: &SettingValue) -> glib::Variant {
    let mut map: HashMap<String, glib::Variant> = entries(dict).collect();
    map.insert(key.to_owned(), value_to_variant(value));
    map.to_variant()
}

/// The `(key, value)` pairs of an `a{sv}`, the values unboxed.
fn entries(dict: &glib::Variant) -> impl Iterator<Item = (String, glib::Variant)> + '_ {
    let items = if dict.is_container() {
        dict.n_children()
    } else {
        0
    };
    (0..items).filter_map(move |i| {
        let entry = dict.child_value(i);
        let key = entry.child_value(0).str()?.to_owned();
        let value = entry.child_value(1).as_variant()?;
        Some((key, value))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> glib::Variant {
        let dict: HashMap<String, glib::Variant> = [
            ("account.mark-watched".to_owned(), false.to_variant()),
            ("dj.pick".to_owned(), "last".to_variant()),
            ("dj.count".to_owned(), 3i32.to_variant()),
        ]
        .into();
        dict.to_variant()
    }

    #[test]
    fn dictionary_becomes_values() {
        let values = extension_values(&dict());
        assert_eq!(values.len(), 2);
        assert_eq!(values["account.mark-watched"], SettingValue::Bool(false));
        assert_eq!(values["dj.pick"], SettingValue::Text("last".into()));
    }

    #[test]
    fn empty_or_wrong_variant_gives_no_values() {
        assert!(extension_values(&HashMap::<String, glib::Variant>::new().to_variant()).is_empty());
        assert!(extension_values(&true.to_variant()).is_empty());
    }

    #[test]
    fn values_become_variants_of_the_right_type() {
        assert_eq!(value_to_variant(&SettingValue::Bool(true)).type_(), "b");
        assert_eq!(
            value_to_variant(&SettingValue::Text("x".into())).type_(),
            "s"
        );
    }

    #[test]
    fn setting_a_value_keeps_the_others_and_stays_a_dictionary() {
        let updated = with_value(&dict(), "account.mark-watched", &SettingValue::Bool(true));
        assert_eq!(updated.type_(), "a{sv}");
        let values = extension_values(&updated);
        assert_eq!(values["account.mark-watched"], SettingValue::Bool(true));
        assert_eq!(values["dj.pick"], SettingValue::Text("last".into()));
        // The entry of an unknown type survives the round trip.
        assert_eq!(updated.n_children(), 3);

        let added = with_value(&updated, "dj.name", &SettingValue::Text("Ana".into()));
        assert_eq!(
            extension_values(&added)["dj.name"],
            SettingValue::Text("Ana".into())
        );
        assert_eq!(added.n_children(), 4);
    }

    #[test]
    fn setting_a_value_in_an_empty_dictionary() {
        let empty = HashMap::<String, glib::Variant>::new().to_variant();
        let updated = with_value(&empty, "a.b", &SettingValue::Bool(true));
        assert_eq!(extension_values(&updated)["a.b"], SettingValue::Bool(true));
    }
}
