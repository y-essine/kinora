//! The extensions that ship with Kinora.
//!
//! - [`account`]: the YouTube account: Home, personal Up next, marking
//!   videos watched, importing its subscriptions.
//! - [`home_chips`]: the filter chips above Home ("Music", "Gaming", ...).
//! - [`dj`]: endless music from a seed with crossfades (the engine is
//!   [`crate::dj`]).
//!
//! Switching `account` and `home-chips` off is Kinora's fully anonymous
//! experience.

pub mod account;
pub mod dj;
pub mod home_chips;

use std::sync::Arc;

use super::Extension;

/// Every built-in extension, in the order they contribute.
pub fn all() -> Vec<Arc<dyn Extension>> {
    vec![
        Arc::new(account::AccountExtension::new()),
        Arc::new(home_chips::HomeChipsExtension::new()),
        Arc::new(dj::DjExtension::new()),
    ]
}
