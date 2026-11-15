//! Kinora: `kinora --self-test`, a check of the whole app on the machine it
//! runs on, with nobody watching. Made for the Windows build, where CI has a
//! real Windows desktop but nobody to click: it checks the bundle (settings
//! schema, translations, enhancers, helpers), plays media through Clapper the
//! way the app does (a `clapper_gtk::Video`, silent), opens the main window and
//! saves screenshots. Every step prints one line; the exit code is the number
//! of required checks that failed.
//!
//!     kinora --self-test [--out DIR] [--media URI]... [--online URL]
//!
//! `--media` plays a URI that must work (a local clip, say). `--online` plays a
//! URL through Clapper's enhancers (yt-dlp) and runs `kinora-prepare` on it;
//! sites may refuse a data centre, so these only warn. The report goes to
//! stdout and to `DIR/self-test.txt`, screenshots to `DIR/*.png` (DIR defaults
//! to the current folder).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use clapper_gtk::prelude::AvExt;
use gtk::gdk::prelude::*;
use gtk::gsk::prelude::*;
use gtk::prelude::*;

/// What `--self-test` was asked to do.
pub struct Options {
    out: PathBuf,
    media: Vec<String>,
    online: Option<String>,
}

/// `Some` when the arguments ask for a self-test.
pub fn options() -> Option<Options> {
    let mut args = std::env::args().skip(1);
    if !std::env::args().any(|a| a == "--self-test") {
        return None;
    }
    let mut options = Options {
        out: PathBuf::from("."),
        media: Vec::new(),
        online: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => options.out = args.next().map(PathBuf::from).unwrap_or_default(),
            "--media" => options.media.extend(args.next()),
            "--online" => options.online = args.next(),
            _ => {}
        }
    }
    Some(options)
}

/// The report: lines to stdout and the report file, and the failures counted.
#[derive(Clone)]
struct Report(Rc<RefCell<(Vec<String>, i32)>>);

impl Report {
    fn line(&self, status: &str, what: &str, detail: impl AsRef<str>) {
        let line = format!("[{status}] {what}: {}", detail.as_ref());
        println!("{line}");
        log::info!("self-test {line}");
        let mut inner = self.0.borrow_mut();
        inner.0.push(line);
        if status == "FAIL" {
            inner.1 += 1;
        }
    }
    fn ok(&self, what: &str, detail: impl AsRef<str>) {
        self.line("ok", what, detail);
    }
    fn fail(&self, what: &str, detail: impl AsRef<str>) {
        self.line("FAIL", what, detail);
    }
    fn warn(&self, what: &str, detail: impl AsRef<str>) {
        self.line("warn", what, detail);
    }
    fn check(&self, required: bool, what: &str, result: Result<String, String>) {
        match result {
            Ok(detail) => self.ok(what, detail),
            Err(detail) if required => self.fail(what, detail),
            Err(detail) => self.warn(what, detail),
        }
    }
}

/// Checks that need nothing but the process: run before GTK starts.
fn environment(report: &Report) {
    report.ok(
        "version",
        format!("{} {}", crate::config::APP_ID, env!("CARGO_PKG_VERSION")),
    );
    report.ok(
        "executable",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|e| e.to_string()),
    );
    for var in [
        "CLAPPER_ENHANCERS_PATH",
        "PYTHONHOME",
        "GI_TYPELIB_PATH",
        "GSK_RENDERER",
    ] {
        report.ok(
            var,
            std::env::var(var).unwrap_or_else(|_| "(not set)".into()),
        );
    }
    let schema = gtk::gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(crate::config::BASE_ID, true));
    report.check(
        true,
        "settings schema",
        schema
            .map(|_| crate::config::BASE_ID.to_string())
            .ok_or_else(|| format!("{} not found", crate::config::BASE_ID)),
    );
    let locale = crate::locale_dir();
    report.check(
        false,
        "translations",
        if locale.join("fr").join("LC_MESSAGES").exists() {
            Ok(locale.display().to_string())
        } else {
            Err(format!("none in {}", locale.display()))
        },
    );
}

/// Run the self-test inside the running application, then quit with the
/// number of failures as the exit code.
pub fn run(app: &gtk::Application, options: Options) {
    let report = Report(Rc::new(RefCell::new((Vec::new(), 0))));
    environment(&report);
    let app = app.clone();
    let hold = app.hold();
    glib::MainContext::default().spawn_local(async move {
        checks(&app, &options, &report).await;
        let (lines, failures) = report.0.borrow().clone();
        let summary = format!("self-test: {failures} failed");
        println!("{summary}");
        let _ = std::fs::create_dir_all(&options.out);
        let text = lines.join("\n") + "\n" + &summary + "\n";
        if let Err(e) = std::fs::write(options.out.join("self-test.txt"), text) {
            eprintln!("could not write the report: {e}");
        }
        drop(hold);
        // Helpers and players may still hold threads; this is the end.
        std::process::exit(failures);
    });
}

async fn checks(app: &gtk::Application, options: &Options, report: &Report) {
    let _ = std::fs::create_dir_all(&options.out);

    // Clapper's enhancers: yt-dlp is how most videos play.
    match clapper::global_enhancer_proxies() {
        Some(proxies) => {
            for (module, required) in [("clapper_yt_dlp", true), ("clapper-peertube", false)] {
                report.check(
                    required,
                    "enhancer",
                    proxies
                        .proxy_by_module(module)
                        .map(|_| module.to_string())
                        .ok_or_else(|| format!("{module} not found")),
                );
            }
        }
        None => report.fail("enhancers", "Clapper has no enhancer list"),
    }

    // The helpers, through the bundled Python.
    let version = crate::TOKIO_RUNTIME
        .spawn(async {
            kinora_core::helper::command("yt-dlp")
                .arg("--version")
                .output()
                .await
        })
        .await;
    report.check(true, "yt-dlp", command_result(version));

    for uri in &options.media {
        let result = play(uri, &options.out, "media", Duration::from_secs(60)).await;
        report.check(true, &format!("play {uri}"), result);
    }

    if let Some(url) = &options.online {
        let out = options.out.join("prepared");
        let url_owned = url.clone();
        let prepared = crate::TOKIO_RUNTIME
            .spawn(async move {
                kinora_core::helper::command("kinora-prepare")
                    .arg("--audio")
                    .arg(&url_owned)
                    .arg(&out)
                    .output()
                    .await
            })
            .await;
        report.check(false, "kinora-prepare", command_result(prepared));
        let result = play(url, &options.out, "online", Duration::from_secs(90)).await;
        report.check(false, &format!("play {url}"), result);
    }

    // The main window, as a user sees it first.
    crate::build_ui(app);
    glib::timeout_future(Duration::from_secs(12)).await;
    match app.active_window().or_else(|| app.windows().into_iter().next()) {
        Some(window) => report.check(
            true,
            "main window",
            screenshot(&window, &options.out.join("window.png")),
        ),
        None => report.fail("main window", "no window"),
    }
}

fn command_result(
    joined: Result<std::io::Result<std::process::Output>, tokio::task::JoinError>,
) -> Result<String, String> {
    match joined {
        Ok(Ok(out)) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap_or("")
            .trim()
            .to_string()),
        Ok(Ok(out)) => Err(format!(
            "{}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | ")
        )),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Play `uri` the way the app does, in a window of its own and silent, until
/// three seconds have played; screenshot it then.
async fn play(uri: &str, out: &Path, name: &str, timeout: Duration) -> Result<String, String> {
    let video = clapper_gtk::Video::new();
    let player = video.player().ok_or("the video has no player")?;
    // No sound card on a CI machine: the sound goes nowhere, decoded all the same.
    if let Ok(sink) = gst::ElementFactory::make("fakesink")
        .property("sync", true)
        .build()
    {
        player.set_property("audio-sink", &sink);
    }
    player.set_mute(true);

    let error: Rc<RefCell<Option<String>>> = Rc::default();
    let e = error.clone();
    player.connect_local("error", false, move |values| {
        let message = values
            .get(1)
            .and_then(|v| v.get::<glib::Error>().ok())
            .map(|err| err.to_string())
            .unwrap_or_else(|| "playback error".into());
        let debug = values
            .get(2)
            .and_then(|v| v.get::<Option<String>>().ok())
            .flatten()
            .unwrap_or_default();
        *e.borrow_mut() = Some(format!("{message} ({debug})"));
        None
    });

    let window = gtk::Window::builder()
        .title(format!("Kinora self-test: {name}"))
        .default_width(640)
        .default_height(360)
        .child(&video)
        .build();
    window.present();

    let queue = player.queue().ok_or("the player has no queue")?;
    queue.add_item(&clapper::MediaItem::new(uri));
    player.play();

    let started = Instant::now();
    let result = loop {
        glib::timeout_future(Duration::from_millis(250)).await;
        if let Some(e) = error.borrow().clone() {
            break Err(e);
        }
        let position = player.position();
        if position >= 3.0 {
            let shot = screenshot(&window, &out.join(format!("{name}.png")))
                .map(|p| format!(", screenshot {p}"))
                .unwrap_or_else(|e| format!(", no screenshot ({e})"));
            break Ok(format!(
                "{position:.1} s played after {:.1} s{shot}",
                started.elapsed().as_secs_f64()
            ));
        }
        if started.elapsed() > timeout {
            break Err(format!(
                "only {position:.1} s played in {} s (state {:?})",
                timeout.as_secs(),
                player.state()
            ));
        }
    };
    player.stop();
    window.close();
    result
}

/// Save what `widget` shows as a PNG.
fn screenshot(widget: &impl IsA<gtk::Widget>, path: &Path) -> Result<String, String> {
    let widget = widget.as_ref();
    let (width, height) = (widget.width(), widget.height());
    if width == 0 || height == 0 {
        return Err("the window has no size (never shown?)".into());
    }
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let node = snapshot.to_node().ok_or("nothing drawn")?;
    let renderer = widget
        .native()
        .and_then(|native| native.renderer())
        .ok_or("no renderer")?;
    let texture = renderer.render_texture(&node, None);
    texture
        .save_to_png(path)
        .map_err(|e| format!("saving {}: {e}", path.display()))?;
    Ok(format!("{} ({width}x{height})", path.display()))
}
