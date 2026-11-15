# Where Kinora is (handoff, 2026-11-15)

For whoever continues (a Claude session on Windows or in the cloud included).
Read this, then `docs/architecture.md`, `docs/design.md`, `CONTRIBUTING-KINORA.md`.

## State
- `main` builds and tests clean on Linux (`cargo test --workspace`). Latest
  release: `v0.1.0-beta.5` (Arch package built locally; no GitHub release
  object yet).
- Done: kinora-core split + extensions; redesign (Home banner/rows, ambient
  player); player sheet + mini-player (YouTube-mobile style); Previous/Next;
  Extensions page in Preferences; DJ (YouTube Music radio, playlists, mixes,
  Liked/Watch later, crossfades on every change, audio-first with artwork,
  warm next track, hover preload); first-run "How do you want to watch?";
  buffered segment on the seek bar; `kinora-prepare` (`--audio`) skips dead
  HLS renditions.

## In progress: the Windows build
- `.github/workflows/windows.yml` (MSYS2 UCRT64): builds Clapper 0.10.0 and
  clapper-enhancers 0.10.0 (yt-dlp, PeerTube) from source, then Kinora, then
  bundles GTK, GStreamer, Python + yt-dlp + PyGObject into
  `Kinora-<version>-windows-x86_64.zip` (artifact). On a `v*` tag push it
  also creates a GitHub pre-release with the zip.
- First run: https://github.com/y-essine/kinora/actions/runs/43761254918 — not
  known yet whether it passes. Next steps:
  1. `gh run view <id> --log-failed`, fix, push, `gh workflow run windows.yml`.
  2. When green: bump the version (Cargo.toml, meson.build, PKGBUILD pkgver,
     a `<release>` in data/*.metainfo.xml.in.in), commit, tag `v0.1.0-beta.6`,
     push the tag; the workflow publishes the release.
  3. Test the zip on Windows: unzip, run `bin\kinora.exe`; log at
     `%LOCALAPPDATA%\kinora\kinora.log`.
- Windows code paths: `kinora_core::helper` (helpers run through the bundled
  `python.exe`, no console; cookies command via `cmd /C`), `windows_environment()`
  in `src/main.rs` (CLAPPER_ENHANCERS_PATH, PYTHONHOME, GI_TYPELIB_PATH from the
  app folder), `kinora-account` Windows browser folders. Untested on Windows.
- Expected limits: Chrome/Edge/Brave cookies on Windows use app-bound
  encryption that yt-dlp can't read (Firefox should work); no media keys
  (Clapper's MPRIS only drives its own queue; also no MPRIS on Windows).

## Open items
- A Clapper crash (SIGSEGV in `g_main_context_unref`, reactables thread) seen
  once when a just-made player was dropped; `VideoPage::retire` now keeps
  let-go pages >= 3 s and >= 10 s after creation. Watch for it.
- Not exercised by hand: drag/swipe gestures and their thresholds, DJ queue
  drag-reorder, real pointer hover preload, crossfade "Off", narrow layouts.
- Own MPRIS service (media keys, desktop Next/Previous for the DJ).
- "See all" on topic rows; Home without an account; playback positions for
  "Continue watching"; memory use (~400 MB); app icon and store screenshots.

## Working agreements
- Commits as `y-essine <60329454+y-essine@users.noreply.github.com>`, message
  ending with a `Co-Authored-By:` trailer for the model. Never the real email.
- Creative/design work goes to the Fable model; tests, builds and routine
  chores to cheaper models.
- Account features: use a separate (burner) YouTube account; unofficial
  clients are against YouTube's terms. Never print cookie values.
- Keep debug logging and caches.
