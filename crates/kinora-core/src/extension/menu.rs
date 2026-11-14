//! Menu contributions: actions an extension offers, which the frontend puts
//! into its own menus.

use pcore::{Channel, Video};
use serde::{Deserialize, Serialize};

/// Which menu an item goes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuLocation {
    /// The app's main menu.
    Main,
    /// The Home page's menu (the account menu in the GTK app).
    Home,
    /// A video's context menu.
    Video,
}

/// An action in a menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItem {
    /// Unique within the extension, e.g. `import-subscriptions`.
    pub id: String,
    pub label: String,
    pub location: MenuLocation,
}

/// What a menu item was activated on.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuTarget {
    None,
    /// The video of a [`MenuLocation::Video`] menu.
    Video(Box<Video>),
}

/// What the frontend should do after an action.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionOutcome {
    /// Nothing more to do.
    Done,
    /// Tell the user this (a toast).
    Message(String),
    /// Subscribe to these channels (the frontend owns the subscriptions).
    Subscribe(Vec<Channel>),
    /// Play this video.
    Play(Box<Video>),
}
