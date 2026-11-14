//! Extensions: optional features the user can switch on and off.
//!
//! An extension does not draw UI. It describes itself in a [`Manifest`]
//! (with the [`Permission`]s it needs and the [`Setting`]s it has) and
//! *contributes* to fixed points the frontends know how to show:
//!
//! - [`FeedSource`]s supply Home with videos and chips; [`FeedFilter`]s adjust
//!   each Home page;
//! - [`PlayerHook`]s react to playback (watched, ended) and choose what plays
//!   next and how (crossfade);
//! - [`MenuItem`]s are actions in the frontend's menus, run by
//!   [`Extension::activate`];
//! - the settings become a [`SettingsPanel`] the frontend renders with its own
//!   widgets.
//!
//! The [`ExtensionRegistry`] holds the extensions, knows which are disabled,
//! and hands each one a [`Context`] with only what its permissions allow.
//! Today all extensions are built in ([`builtin`]); see `docs/architecture.md`
//! for how third-party ones would fit.

pub mod builtin;
mod feed;
mod hooks;
mod menu;
mod registry;
mod settings;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt;

use futures::future::{BoxFuture, ready};
use serde::{Deserialize, Serialize};

pub use feed::{FeedFilter, FeedPlacement, FeedRequest, FeedSource};
pub use hooks::PlayerHook;
pub use menu::{ActionOutcome, MenuItem, MenuLocation, MenuTarget};
pub use registry::ExtensionRegistry;
pub use settings::{Setting, SettingChoice, SettingKind, SettingValue, SettingsPanel, value_key};

use crate::{AccountConfig, Error, Paths};

/// What an extension may do. Shown to the user, and checked whenever an
/// extension asks its [`Context`] for something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Talk to servers on the internet.
    Network,
    /// Use the connected YouTube account (its cookies).
    Account,
    /// Decide what plays next and how ([`PlayerHook::choose_next`],
    /// [`PlayerHook::transition`]).
    PlayerControl,
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Network => "network",
            Self::Account => "account",
            Self::PlayerControl => "player control",
        })
    }
}

/// Who an extension is and what it needs. Built-in extensions write theirs
/// in code; for third-party ones it would be a file shipped with them, hence
/// the serde support.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Unique and stable (it is stored in the disabled list and in setting
    /// keys), e.g. `account`. Lowercase letters, digits and `-`.
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub settings: Vec<Setting>,
    /// Shipped with Kinora.
    #[serde(default)]
    pub builtin: bool,
}

impl Manifest {
    pub fn has(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    pub fn setting(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.key == key)
    }

    /// Whether the manifest is well-formed: valid ids, unique setting keys,
    /// choice defaults among their options.
    pub fn validate(&self) -> Result<(), Error> {
        if !is_id(&self.id) {
            return Err(format!("invalid extension id {:?}", self.id).into());
        }
        let mut keys = std::collections::HashSet::new();
        for setting in &self.settings {
            if !is_id(&setting.key) || !keys.insert(setting.key.as_str()) {
                return Err(
                    format!("{}: invalid or repeated setting {:?}", self.id, setting.key).into(),
                );
            }
            if !setting.kind.accepts(&setting.kind.default_value()) {
                return Err(format!("{}: {} has an invalid default", self.id, setting.key).into());
            }
        }
        Ok(())
    }
}

fn is_id(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// An optional feature. Implement [`Extension::manifest`] and whichever
/// contributions apply; the registry asks for them once, when the extension
/// is registered.
pub trait Extension: Send + Sync {
    fn manifest(&self) -> &Manifest;

    fn feed_sources(&self) -> Vec<std::sync::Arc<dyn FeedSource>> {
        Vec::new()
    }

    fn feed_filters(&self) -> Vec<std::sync::Arc<dyn FeedFilter>> {
        Vec::new()
    }

    fn player_hooks(&self) -> Vec<std::sync::Arc<dyn PlayerHook>> {
        Vec::new()
    }

    fn menu_items(&self) -> Vec<MenuItem> {
        Vec::new()
    }

    /// Run one of this extension's [`MenuItem`]s.
    fn activate(
        &self,
        _ctx: Context,
        item: &str,
        _target: MenuTarget,
    ) -> BoxFuture<'static, Result<ActionOutcome, Error>> {
        let error = format!("{}: no menu item {item:?}", self.manifest().id);
        Box::pin(ready(Err(error.into())))
    }
}

/// What an extension gets to work with, built by the registry for each call:
/// its own settings, and the app's state as far as its permissions allow.
#[derive(Debug, Clone)]
pub struct Context {
    extension: String,
    permissions: Vec<Permission>,
    account: AccountConfig,
    paths: Paths,
    values: HashMap<String, SettingValue>,
}

impl Context {
    pub fn extension(&self) -> &str {
        &self.extension
    }

    pub fn has(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    /// `Err` unless the extension declared `permission`.
    pub fn require(&self, permission: Permission) -> Result<(), Error> {
        if self.has(permission) {
            Ok(())
        } else {
            Err(format!(
                "The {} extension lacks the {permission} permission",
                self.extension
            )
            .into())
        }
    }

    /// The YouTube account (needs [`Permission::Account`]).
    pub fn account(&self) -> Result<&AccountConfig, Error> {
        self.require(Permission::Account)?;
        Ok(&self.account)
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// The current value of one of the extension's settings (its default
    /// unless the user changed it).
    pub fn value(&self, key: &str) -> Option<&SettingValue> {
        self.values.get(key)
    }

    /// A toggle's value; `false` for unknown keys.
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.value(key), Some(SettingValue::Bool(true)))
    }

    /// A choice's or text's value; `""` for unknown keys.
    pub fn text(&self, key: &str) -> &str {
        match self.value(key) {
            Some(SettingValue::Text(t)) => t,
            _ => "",
        }
    }
}
