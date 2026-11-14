//! Kinora: a one-time import from Pipeline (and Pipeline Home), which kept the
//! same library and settings under the "tubefeeder" names. Nothing is moved:
//! Pipeline's own files stay as they are, so both apps can live side by side.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::BASE_ID;

const OLD_DIR: &str = "tubefeeder";
const OLD_SETTINGS: &str = "/de/schmidhuberj/tubefeeder/";
pub const DIR: &str = "kinora";

pub fn from_pipeline() {
    library();
    settings();
}

/// Copy Pipeline's library (subscriptions, watch later, history) on first start.
fn library() {
    let data = glib::user_data_dir();
    let (old, new) = (data.join(OLD_DIR), data.join(DIR));
    if new.exists() || !old.join("data.sqlite").exists() {
        return;
    }
    match copy_dir(&old, &new) {
        Ok(()) => log::info!("Imported the library from {}", old.display()),
        Err(e) => log::warn!("Could not import the library from {}: {e}", old.display()),
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Copy Pipeline's settings (the keys are the same) while Kinora has none yet.
fn settings() {
    // Only with the real settings store (dconf); a development build running on
    // a keyfile or in memory must not write into the installed app's settings.
    if std::env::var("GSETTINGS_BACKEND").is_ok_and(|b| b != "dconf") {
        return;
    }
    let new = format!("/{}/", BASE_ID.replace('.', "/"));
    let dump = |path: &str| {
        Command::new("dconf")
            .args(["dump", path])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    if !dump(&new).trim().is_empty() {
        return;
    }
    let old = dump(OLD_SETTINGS);
    if old.trim().is_empty() {
        return;
    }
    // Pipeline Home's account helper was renamed.
    let migrated = old.replace("yt-account-cookies", "kinora-account");
    let loaded = Command::new("dconf")
        .args(["load", &new])
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .expect("stdin to be piped")
                .write_all(migrated.as_bytes())?;
            child.wait()
        });
    match loaded {
        Ok(status) if status.success() => log::info!("Imported the settings from Pipeline"),
        Ok(status) => log::warn!("Could not import the settings from Pipeline: dconf {status}"),
        Err(e) => log::warn!("Could not import the settings from Pipeline: {e}"),
    }
}
