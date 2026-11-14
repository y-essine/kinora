use gdk::{
    Display,
    gio::prelude::SettingsExt,
    prelude::{ApplicationExt, ApplicationExtManual},
};
use glib::object::IsA;
use gtk::{CssProvider, prelude::GtkWindowExt, prelude::RootExt};
use std::sync::LazyLock;

mod config;
mod migrate;
use crate::config::BASE_ID;

use self::config::{APP_ID, GETTEXT_PACKAGE, LOCALEDIR, RESOURCES_BYTES, RESOURCES_PATH};

mod backend;
mod gui;
mod pride;
mod splashes;
mod store;

pub static TOKIO_RUNTIME: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Runtime::new().unwrap());

#[macro_export]
macro_rules! gspawn {
    ($future:expr) => {
        let ctx = glib::MainContext::default();
        ctx.spawn_local($future);
    };
    ($future:expr, $priority:expr) => {{
        let ctx = gtk::glib::MainContext::default();
        ctx.spawn_local_with_priority($priority, $future)
    }};
}
#[macro_export]
macro_rules! gspawn_global {
    ($future:expr) => {
        let ctx = glib::MainContext::default();
        ctx.spawn($future);
    };
}

#[macro_export]
macro_rules! tspawn {
    ($future:expr) => {
        $crate::TOKIO_RUNTIME.spawn($future)
    };
}

fn init_resources() {
    let gbytes = gtk::glib::Bytes::from_static(RESOURCES_BYTES);
    let resource = gtk::gio::Resource::from_data(&gbytes).unwrap();

    gtk::gio::resources_register(&resource);
}

fn init_folders() {
    let mut user_cache_dir = gtk::glib::user_cache_dir();
    user_cache_dir.push(migrate::DIR);

    if !user_cache_dir.exists() {
        std::fs::create_dir_all(&user_cache_dir).expect("could not create user cache dir");
    }

    let mut user_data_dir = gtk::glib::user_data_dir();
    user_data_dir.push(migrate::DIR);

    if !user_data_dir.exists() {
        std::fs::create_dir_all(user_data_dir.clone()).expect("could not create user data dir");
    }
}

fn init_internationalization() -> Result<(), Box<dyn std::error::Error>> {
    // Safety: Single-threaded (at this point).
    unsafe {
        gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    }
    gettextrs::bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR)?;
    gettextrs::textdomain(GETTEXT_PACKAGE)?;
    Ok(())
}

fn init_icons<P: IsA<gdk::Display>>(display: &P) {
    let icon_theme = gtk::IconTheme::for_display(display);

    icon_theme.add_resource_path(&format!("{RESOURCES_PATH}icons/"));
}

fn init_css() {
    let provider = CssProvider::new();
    provider.load_from_resource(&format!("{RESOURCES_PATH}style.css"));

    // Add the provider to the default screen
    gtk::style_context_add_provider_for_display(
        &Display::default().expect("Could not connect to a display."),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn main() {
    env_logger::init();
    // Kinora: bring over the library and settings from Pipeline, once.
    migrate::from_pipeline();

    // Before version 4.18, the NGL renderer is not so good, and Pipeline defaults to the old GL renderer.
    // Since 4.18, the old renderer was removed, and we will have to use the default.
    // See their annoucement of removal https://blog.gtk.org/2025/02/01/whats-new-in-gtk-winter-2025-edition/
    if gtk::major_version() == 4 && gtk::minor_version() < 18 {
        log::info!(
            "Running with an older version of GTK. Using the older renderers for better performance"
        );
        // Copied from https://gitlab.gnome.org/GNOME/loupe/-/commit/0757bf5a5cbb034a44b9ce03c3784a2eb1169cb2#
        // Don't use ngl renderer by default due to performance reasons
        // <https://gitlab.gnome.org/GNOME/gtk/-/issues/6411>
        // <https://github.com/GeopJr/Tuba/pull/847>
        if std::env::var("GSK_RENDERER").map_or(true, |x| x.is_empty()) {
            // Safety: At this point the application is still single-threaded.
            unsafe {
                std::env::set_var("GSK_RENDERER", "gl");
            }
        }
        // Don't use gl renderer does not properly support fractional scaling
        // <https://gitlab.gnome.org/GNOME/loupe/-/issues/346>
        if std::env::var("GDK_DEBUG").is_err() {
            // Safety: At this point the application is still single-threaded.
            unsafe {
                std::env::set_var("GDK_DEBUG", "gl-no-fractional");
            }
        }
    }

    if gtk::gio::Settings::new(BASE_ID).boolean("cairo-software-renderer") {
        // Safety: At this point the application is still single-threaded.
        unsafe {
            std::env::set_var("GSK_RENDERER", "cairo");
        }
    }

    init_internationalization().expect("Failed to initialize internationalization");

    gtk::init().expect("Failed to initialize gtk");
    adw::init().expect("Failed to initialize adw");

    // This improves performance of Clapper a lot.
    // It still has some bugs though, that is why it is deactivated in Clapper by default.
    // But as multiple people reported those performance issues compared with the external Clapper player, I'll set this either way.
    glib::setenv("CLAPPER_USE_PLAYBIN3", "1", false).unwrap();
    clapper::init().expect("Failed to initialize clapper");

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .resource_base_path(RESOURCES_PATH)
        .build();
    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &gtk::Application) {
    init_resources();
    init_folders();
    init_css();
    // Create new window and present it
    let window = crate::gui::window::Window::new(app);
    init_icons(&window.display());

    glib::set_prgname(Some("kinora"));
    glib::set_application_name(&gettextrs::gettext("Kinora"));
    window.set_icon_name(Some(APP_ID));

    window.present();
}
