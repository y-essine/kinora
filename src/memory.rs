// SPDX-License-Identifier: GPL-3.0-or-later

//! Kinora: hand memory the app has freed back to the system.
//!
//! glibc's allocator keeps what is freed for later use, in an arena per
//! thread that allocated (up to eight per core). A video or a DJ track
//! allocates a lot on its streaming and decoding threads (buffers, decoders,
//! the DJ's PCM), and all of it is freed when its player goes, but the pages
//! stay counted in the app's memory: only the top of an arena is returned by
//! itself. `malloc_trim` returns the free pages of every arena.

use std::cell::Cell;

use gtk::glib;

/// After this long, [`trim_soon`] trims.
const SOON: std::time::Duration = std::time::Duration::from_secs(5);

thread_local! {
    static PENDING: Cell<bool> = const { Cell::new(false) };
}

/// [`trim`] a few seconds from now, once for any number of calls meanwhile:
/// after a player was let go (its threads free its memory as they end) and
/// after Home was read (its pages are parsed on the Tokio threads). A trim
/// takes about 10 ms and locks one arena at a time, briefly.
pub fn trim_soon() {
    if PENDING.with(|p| p.replace(true)) {
        return;
    }
    glib::timeout_add_local_once(SOON, || {
        PENDING.with(|p| p.set(false));
        trim();
    });
}

/// The resident memory of this process, in kB (Linux), for the log.
pub fn resident_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
}

/// Return the free pages of every malloc arena to the system.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn trim() {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> std::ffi::c_int;
    }
    let (before, began) = (resident_kb(), std::time::Instant::now());
    // SAFETY: malloc_trim only takes the allocator's own locks.
    unsafe { malloc_trim(0) };
    if let (Some(before), Some(after)) = (before, resident_kb()) {
        log::debug!(
            "Memory: {} MB given back ({} MB resident, in {} ms)",
            before.saturating_sub(after) / 1024,
            after / 1024,
            began.elapsed().as_millis()
        );
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn trim() {}
