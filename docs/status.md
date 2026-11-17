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

## The Windows build (green on `fix/windows-ci`, 2026-11-16)
- `.github/workflows/windows.yml` (MSYS2 UCRT64): builds Clapper 0.10.0 and
  clapper-enhancers 0.10.0 (yt-dlp, PeerTube) from source, then Kinora, then
  bundles GTK, GStreamer, Python + yt-dlp + PyGObject into
  `Kinora-<version>-windows-x86_64.zip` (artifact `kinora-windows`, made
  whenever the bundle builds). On a `v*` tag push, with every step green, it
  also creates a GitHub pre-release with the zip.
- Run by hand: `gh workflow run windows.yml --ref <branch>`, then
  `gh run view <id> --log-failed`. A run takes ~15 min.
- **`kinora --self-test`** (`src/selftest.rs`, in every build): checks the
  settings schema, translations, Clapper's enhancers, yt-dlp, the account
  cookies command (dummy cookies.txt through the system shell), plays
  `--media URI`s (required) and an `--online URL` (warn only), opens the main
  window and saves screenshots to `--out DIR`. Exit code = failures. CI runs it
  with only the bundle visible (`env -i`) and uploads `kinora-windows-self-test`
  (report, PNGs, `kinora-self-test.log`). The runner has no GPU, so CI tests a
  copy of the bundle with Mesa's llvmpipe `opengl32.dll`; the zip has none.
  YouTube refuses GitHub's IPs ("confirm you're not a bot"): online is warn-only.
- Tested on a real Windows 11 PC (zip from run 43776143983): every check ok,
  YouTube plays through Clapper + yt-dlp (~9-13 s to the first frame),
  `kinora-prepare` works, browser profiles (Chrome, Firefox) are found.
- What it took (all in this branch): Clapper 0.10.0 misses DLL exports the
  Rust bindings use and builds MPRIS only on Linux —
  `build-aux/windows/patch-clapper.sh` patches that (each edit checked; a new
  Clapper may need it updated or dropped). ninja runs commands without a shell
  on Windows: `build-aux/cargo-build.sh`. DLLs are collected from the import
  tables (`build-aux/windows/check-bundle.sh`, also the smoke test; ldd missed
  libgirepository-2.0). Translations are found beside kinora.exe
  (`locale_dir()`), not at the build path. The account's cookies command runs
  through `cmd /S /C` with `.cmd` launchers (kinora-account.cmd, ...) and
  double quotes. The player's error handler no longer aborts when Clapper drops
  it on its own thread ("Value dropped on a different thread").
- Every page change and every toast crashed (access violation at address 0):
  MSYS2's GTK 4.24 is built without AccessKit, so widgets get GTK's test
  accessibility context, whose `announce` is a null pointer, and
  `gtk_accessible_announce` (Kinora's page announcements, every AdwToast) calls
  it. `windows_environment()` sets `GTK_A11Y=none` (no context: announce is
  skipped). With an AccessKit-enabled GTK, use `GTK_A11Y=accesskit` instead.
- Native crashes on Windows are written to `%LOCALAPPDATA%\kinora\crash.txt`
  (`src/crashlog.rs`): registers and every stack address in a module as
  `module+offset`. Resolve them against the same build's files (DLL export
  tables, kinora.exe's symbol table), e.g. `objdump -d --start-address=` or a
  small PE export lookup; with rip 0 the first stack entry is the caller.
  Panics go to the log with a backtrace. CI also builds `kinora-dev.exe` (dev
  hooks, artifact `kinora-dev-exe`): drop it into a bundle's `bin\`.
- Next: merge `fix/windows-ci`; bump the version (Cargo.toml, meson.build,
  PKGBUILD pkgver, a `<release>` in data/*.metainfo.xml.in.in), tag
  `v0.1.0-beta.6`, push the tag; the workflow publishes the release.
- Windows code paths: `kinora_core::helper` (helpers run through the bundled
  `python.exe`, no console), `windows_environment()` and `locale_dir()` in
  `src/main.rs`, `kinora-account` Windows browser folders. Log:
  `%LOCALAPPDATA%\kinora\kinora.log`.
- Expected limits: Chrome/Edge/Brave cookies on Windows use app-bound
  encryption that yt-dlp can't read (Firefox works, or a cookies.txt); no
  media keys (no MPRIS on Windows: it is built but finds no session bus, hence
  the harmless "win32 session dbus binary not found" warning). Not yet tried by
  hand on Windows: DJ crossfades, downloads, the external player, fullscreen.

## Open items
- A Clapper crash (SIGSEGV in `g_main_context_unref`, reactables thread) seen
  once when a just-made player was dropped; `VideoPage::retire` now keeps
  let-go pages >= 3 s and >= 10 s after creation. Watch for it.
- Not exercised by hand: drag/swipe gestures and their thresholds, DJ queue
  drag-reorder, real pointer hover preload, crossfade "Off", narrow layouts.
- Own MPRIS service (media keys, desktop Next/Previous for the DJ).
- "See all" on topic rows; Home without an account; playback positions for
  "Continue watching"; app icon and store screenshots.
- Memory (measured 2026-11-17, release build, headless): ~185 MB at Home,
  ~300 MB with a video, ~320-340 MB in the DJ. What still grows: every video
  page ever made leaves ~11.5 MB and 6 threads behind (11 videos opened one
  after another, then closed: 275 -> 390 MB, 33 -> 92 threads). That is
  Clapper's video sink: each `ClapperGtkVideo` makes a GL context that is
  never freed (`gstclapperglcontexthandler.c`, the commented-out
  `gst_clear_object (&self->gst_context)` and its FIXME, still so on
  Clapper's master). The DJ makes a page per track (and per warm or hovered
  one), so a long session adds up. Fix in Clapper (one GL display and
  context for all sinks, as its FIXME says), or reuse pages. Home's
  thumbnails are kept decoded at full size (720 px wide, ~0.9 MB each) for
  as long as Home holds their videos: ~35 MB for a screenful of rows, more
  as rows are scrolled; decoding cards smaller needs the banner and the
  player's poster to load their own full-size picture.

## Working agreements
- Commits as `y-essine <60329454+y-essine@users.noreply.github.com>`, message
  ending with a `Co-Authored-By:` trailer for the model. Never the real email.
- Creative/design work goes to the Fable model; tests, builds and routine
  chores to cheaper models.
- Account features: use a separate (burner) YouTube account; unofficial
  clients are against YouTube's terms. Never print cookie values.
- Keep debug logging and caches.
