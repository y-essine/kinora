// Kinora: on Windows a GUI app, without a console window (logs go to a file).
#![cfg_attr(windows, windows_subsystem = "windows")]

use gdk::{
    Display,
    gio::prelude::SettingsExt,
    prelude::{ApplicationExt, ApplicationExtManual},
};
use glib::object::IsA;
use gtk::{CssProvider, prelude::GtkWindowExt, prelude::RootExt};
use std::sync::LazyLock;

mod config;
mod crashlog;
mod migrate;
use crate::config::BASE_ID;

use self::config::{APP_ID, GETTEXT_PACKAGE, LOCALEDIR, RESOURCES_BYTES, RESOURCES_PATH};

mod backend;
mod gui;
mod pride;
mod selftest;
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
    // Kinora: development builds can load the UI from the build folder instead
    // of the copy compiled into the program, so a layout change needs only the
    // Blueprint compile and a restart, not a Rust rebuild.
    if let Some(path) = dev_path("KINORA_DEV_RESOURCES") {
        match gtk::gio::Resource::load(&path) {
            Ok(resource) => {
                log::info!("Development: UI from {}", path.display());
                gtk::gio::resources_register(&resource);
                return;
            }
            Err(e) => log::warn!("Development: could not load {}: {e}", path.display()),
        }
    }
    let gbytes = gtk::glib::Bytes::from_static(RESOURCES_BYTES);
    let resource = gtk::gio::Resource::from_data(&gbytes).unwrap();

    gtk::gio::resources_register(&resource);
}

/// Kinora: a file named by an environment variable, honoured only in
/// development builds.
fn dev_path(var: &str) -> Option<std::path::PathBuf> {
    if config::PROFILE != "Devel" {
        return None;
    }
    std::env::var_os(var)
        .map(std::path::PathBuf::from)
        .filter(|p| p.exists())
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
    gettextrs::bindtextdomain(GETTEXT_PACKAGE, locale_dir())?;
    gettextrs::textdomain(GETTEXT_PACKAGE)?;
    Ok(())
}

/// Kinora: where the translations are. The Windows build is a folder that
/// can be anywhere, so there they are found next to `kinora.exe`
/// (`share\locale`), not at the path it was built for.
pub(crate) fn locale_dir() -> std::path::PathBuf {
    #[cfg(windows)]
    if let Some(root) = app_root() {
        return root.join("share").join("locale");
    }
    std::path::PathBuf::from(LOCALEDIR)
}

/// Kinora: the Windows build's folder (the one holding `bin\kinora.exe`).
#[cfg(windows)]
fn app_root() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent()?.parent().map(|p| p.to_path_buf()))
}

fn init_icons<P: IsA<gdk::Display>>(display: &P) {
    let icon_theme = gtk::IconTheme::for_display(display);

    icon_theme.add_resource_path(&format!("{RESOURCES_PATH}icons/"));
}

fn init_css() {
    let provider = CssProvider::new();
    // Kinora: development builds can style from the source file and reload it
    // whenever it is saved: live CSS editing while the app runs.
    if let Some(path) = dev_path("KINORA_DEV_CSS") {
        let file = gtk::gio::File::for_path(&path);
        provider.load_from_file(&file);
        use gtk::gio::prelude::FileExt;
        if let Ok(monitor) = file.monitor_file(
            gtk::gio::FileMonitorFlags::NONE,
            gtk::gio::Cancellable::NONE,
        ) {
            let p = provider.clone();
            use gtk::gio::prelude::FileMonitorExt;
            monitor.connect_changed(move |_, file, _, event| {
                if event == gtk::gio::FileMonitorEvent::ChangesDoneHint {
                    log::info!("Development: reloading CSS");
                    p.load_from_file(file);
                }
            });
            // Keep the monitor alive for the life of the app.
            std::mem::forget(monitor);
        }
        log::info!("Development: live CSS from {}", path.display());
    } else {
        provider.load_from_resource(&format!("{RESOURCES_PATH}style.css"));
    }

    // Add the provider to the default screen
    gtk::style_context_add_provider_for_display(
        &Display::default().expect("Could not connect to a display."),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Kinora: logs. On Windows there is no console: they go to
/// `%LOCALAPPDATA%\\kinora\\kinora.log` (info and up unless RUST_LOG says).
fn init_logging() {
    // Kinora: a panic goes to the log too, with where it happened. On Windows
    // there is no console: without this a crash leaves no trace at all.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log::error!(
            "panic in thread {}: {info}\n{}",
            thread.name().unwrap_or("<unnamed>"),
            std::backtrace::Backtrace::force_capture()
        );
        default_hook(info);
    }));
    #[cfg(windows)]
    {
        let dir = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("kinora");
        let _ = std::fs::create_dir_all(&dir);
        crashlog::install(&dir);
        // A self-test keeps its own log: the app's last session stays readable.
        let name = if std::env::args().any(|a| a == "--self-test") {
            "kinora-self-test.log"
        } else {
            "kinora.log"
        };
        if let Ok(file) = std::fs::File::create(dir.join(name)) {
            env_logger::Builder::from_env(
                env_logger::Env::default().default_filter_or("kinora=info,kinora_core=info"),
            )
            .target(env_logger::Target::Pipe(Box::new(file)))
            .init();
            return;
        }
    }
    // Kinora: development builds also keep the log's tail for the Dev panel.
    #[cfg(feature = "dev-hooks")]
    gui::devtools::install_logger(env_logger::Builder::from_default_env().build());
    #[cfg(not(feature = "dev-hooks"))]
    env_logger::init();
}

/// Kinora: the Windows build is one folder (bin, lib, share). Point Clapper's
/// enhancers (yt-dlp, PeerTube), the Python they run in and GObject
/// introspection at it, unless set already.
#[cfg(windows)]
fn windows_environment() {
    let Some(root) = app_root() else {
        return;
    };
    let vars = [
        ("CLAPPER_ENHANCERS_PATH", root.join("lib/clapper-0.0/enhancers")),
        ("PYTHONHOME", root.clone()),
        ("GI_TYPELIB_PATH", root.join("lib/girepository-1.0")),
        // MSYS2's GTK (4.24) is built without AccessKit, so on Windows every
        // widget gets GTK's "test" accessibility context, whose announce is a
        // null pointer: gtk_accessible_announce() (a page change, every
        // AdwToast) then crashes. With no context at all GTK skips it. Nothing
        // is lost: that GTK has no Windows screen reader support. A GTK with
        // AccessKit should get GTK_A11Y=accesskit here instead.
        ("GTK_A11Y", std::path::PathBuf::from("none")),
    ];
    for (name, value) in vars {
        if std::env::var_os(name).is_none() {
            // SAFETY: first thing in main, before any other thread exists.
            unsafe { std::env::set_var(name, value) };
        }
    }
}

fn main() {
    #[cfg(windows)]
    windows_environment();
    init_logging();
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

    // Kinora: without its settings schema GSettings aborts; say so in the log
    // (and let `--self-test` report it) instead of dying here.
    let schema_found = gtk::gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(BASE_ID, true))
        .is_some();
    if !schema_found {
        log::error!("Settings schema {BASE_ID} not found");
    }
    if schema_found && gtk::gio::Settings::new(BASE_ID).boolean("cairo-software-renderer") {
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
    // Kinora: development builds from several checkouts (git worktrees) run
    // side by side instead of handing over to the one that started first.
    #[cfg(feature = "dev-hooks")]
    {
        use gtk::gio::prelude::ApplicationExt;
        app.set_flags(gtk::gio::ApplicationFlags::NON_UNIQUE);
    }
    // Kinora: `--self-test` checks the build on this machine and quits (see
    // selftest.rs); it never hands over to a Kinora that is already open.
    if let Some(options) = selftest::options() {
        if !schema_found {
            println!("[FAIL] settings schema: {BASE_ID} not found");
            std::process::exit(1);
        }
        app.set_flags(gtk::gio::ApplicationFlags::NON_UNIQUE);
        let options = std::cell::Cell::new(Some(options));
        app.connect_activate(move |app| {
            init_resources();
            init_folders();
            init_css();
            if let Some(options) = options.take() {
                selftest::run(app, options);
            }
        });
        // Its options are not GTK's.
        let argv0: Vec<String> = std::env::args().take(1).collect();
        app.run_with_args(&argv0);
        return;
    }
    if !schema_found {
        // Nothing works without settings: GSettings would abort in the window.
        std::process::exit(1);
    }
    app.connect_activate(build_ui);
    app.run();
}

pub(crate) fn build_ui(app: &gtk::Application) {
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
