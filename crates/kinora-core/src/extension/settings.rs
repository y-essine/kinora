//! Declarative extension settings: an extension says *what* it can be set
//! to, and each frontend renders that with its own widgets (a switch, a
//! combo row, an entry). Extensions never draw UI themselves.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// One setting an extension declares in its [`Manifest`](super::Manifest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    /// Unique within the extension, e.g. `mark-watched`. Lowercase letters,
    /// digits and `-`.
    pub key: String,
    /// The row's title, e.g. "Mark Videos Watched on YouTube".
    pub title: String,
    /// A longer explanation under the title.
    #[serde(default)]
    pub subtitle: Option<String>,
    pub kind: SettingKind,
}

/// The kind of a setting, with its default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum SettingKind {
    /// On or off (a switch).
    Toggle { default: bool },
    /// One of a few values (a combo row or radio buttons).
    Choice {
        options: Vec<SettingChoice>,
        /// The `value` of one of the options.
        default: String,
    },
    /// A whole number in a range (a slider or a spin button). Stored as
    /// text, in decimal, so it can replace a choice of numbers.
    Number {
        min: u32,
        max: u32,
        default: u32,
        /// What the number counts, for the label: "s" gives "6 s".
        #[serde(default)]
        unit: Option<String>,
        /// What to show instead of the minimum, e.g. "Off" for 0.
        #[serde(default)]
        min_label: Option<String>,
    },
    /// Free text (an entry).
    Text {
        default: String,
        #[serde(default)]
        placeholder: Option<String>,
    },
}

/// An option of a [`SettingKind::Choice`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingChoice {
    /// What is stored.
    pub value: String,
    /// What is shown.
    pub label: String,
}

/// A setting's value, as stored by the frontend: toggles are `Bool`, choices,
/// numbers and text are `Text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    Text(String),
}

impl SettingKind {
    pub fn default_value(&self) -> SettingValue {
        match self {
            Self::Toggle { default } => SettingValue::Bool(*default),
            Self::Choice { default, .. } | Self::Text { default, .. } => {
                SettingValue::Text(default.clone())
            }
            Self::Number { default, .. } => SettingValue::Text(default.to_string()),
        }
    }

    /// Whether `value` fits this setting (a stored value of a setting whose
    /// kind changed in a newer version does not).
    pub fn accepts(&self, value: &SettingValue) -> bool {
        match (self, value) {
            (Self::Toggle { .. }, SettingValue::Bool(_)) => true,
            (Self::Choice { options, .. }, SettingValue::Text(v)) => {
                options.iter().any(|o| &o.value == v)
            }
            (Self::Number { min, max, .. }, SettingValue::Text(v)) => {
                v.parse::<u32>().is_ok_and(|n| (*min..=*max).contains(&n))
            }
            (Self::Text { .. }, SettingValue::Text(_)) => true,
            _ => false,
        }
    }
}

impl Setting {
    pub fn toggle(key: &str, title: &str, subtitle: Option<&str>, default: bool) -> Self {
        Self {
            key: key.to_owned(),
            title: title.to_owned(),
            subtitle: subtitle.map(str::to_owned),
            kind: SettingKind::Toggle { default },
        }
    }
}

/// The key a frontend stores a setting under: `<extension id>.<setting key>`
/// (the GTK app keeps them in the `extension-settings` GSettings dictionary).
pub fn value_key(extension: &str, setting: &str) -> String {
    format!("{extension}.{setting}")
}

/// Everything a frontend needs to show one extension in its preferences: a
/// row with an on/off switch, the permissions, and its settings with their
/// current values.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsPanel {
    pub extension: String,
    pub title: String,
    pub description: String,
    pub builtin: bool,
    pub enabled: bool,
    pub permissions: Vec<super::Permission>,
    pub settings: Vec<Setting>,
    /// The current value of every setting in `settings`, by key.
    pub values: HashMap<String, SettingValue>,
}
