//! The extensions that ship with Kinora.
//!
//! - [`account`]: the YouTube account: Home, personal Up next, marking
//!   videos watched, importing its subscriptions.
//! - [`home_chips`]: the filter chips above Home ("Music", "Gaming", ...).
//!
//! Switching both off is Kinora's fully anonymous experience.

pub mod account;
pub mod home_chips;

use std::sync::Arc;

use super::Extension;

/// Every built-in extension, in the order they contribute.
pub fn all() -> Vec<Arc<dyn Extension>> {
    vec![
        Arc::new(account::AccountExtension::new()),
        Arc::new(home_chips::HomeChipsExtension::new()),
    ]
}
