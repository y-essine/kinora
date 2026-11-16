//! Starting Kinora's helper programs (`kinora-account`, `kinora-prepare`,
//! `yt-dlp`) and the account's cookies command on each system.
//!
//! On Linux they are programs on the `PATH`. The Windows build ships them as
//! Python scripts next to `kinora.exe` with a Python of its own, so there they
//! run through that Python, with no console window flashing up.

use tokio::process::Command;

/// A command for one of Kinora's helpers, by its Linux program name.
pub fn command(name: &str) -> Command {
    #[cfg(windows)]
    {
        let dir = exe_dir();
        let python = dir.join("python.exe");
        let mut command = Command::new(&python);
        if name == "yt-dlp" {
            command.args(["-m", "yt_dlp"]);
        } else {
            command.arg(dir.join(name));
        }
        no_window(&mut command);
        command
    }
    #[cfg(not(windows))]
    {
        Command::new(name)
    }
}

/// A command line typed by the user (the account's cookies command).
pub fn shell(line: &str) -> Command {
    #[cfg(windows)]
    {
        // As typed: `cmd /S /C "line"` drops just the outer quotes. Passed as a
        // normal argument, Rust's escaping would turn the line's own quotes
        // into \" , which cmd does not read. The helpers are found as the
        // `.cmd` launchers beside kinora.exe (kinora-account.cmd, ...).
        let mut command = Command::new("cmd");
        command.raw_arg(format!("/S /C \"{line}\""));
        // Its helpers are found next to kinora.exe.
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![exe_dir()];
        paths.extend(std::env::split_paths(&path));
        if let Ok(joined) = std::env::join_paths(paths) {
            command.env("PATH", joined);
        }
        no_window(&mut command);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("sh");
        command.arg("-c").arg(line);
        command
    }
}

#[cfg(windows)]
fn exe_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default()
}

#[cfg(windows)]
fn no_window(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}
