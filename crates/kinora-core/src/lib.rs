//! Kinora's core: everything the app knows that is not about how it looks.
//!
//! No GUI toolkit is used here (no gtk, gdk, glib or adw). Frontends (the GTK
//! app in `src/`) read their own settings, pass plain values in, and turn the
//! results into their own widgets and models. See `docs/architecture.md`.

pub mod account;
pub mod streams;
