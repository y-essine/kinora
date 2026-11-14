# Developing Kinora

Upstream Pipeline's guide is in [CONTRIBUTING.md](CONTRIBUTING.md); this is the fast loop for Kinora.

**Easiest:** `dev/watch.sh` (needs `watchexec`) rebuilds and restarts the development build whenever you save, picking the fastest path below.

| You changed | Run | Takes |
|---|---|---|
| Rust (`src/`) | `dev/build.sh && dev/run.sh` | a few seconds |
| UI (`.blp`) | `dev/ui.sh` | ~1 s + restart, no Rust rebuild |
| CSS (`data/resources/style.css`) | nothing | live, while the app runs |

- **Development build** (`dev/build.sh`): app ID `io.github.y_essine.Kinora.Devel`, built with the `dev-hooks` feature. It runs next to an installed Kinora.
- **Kept apart** (`dev/run.sh`): its own library, cache and settings under `.dev/home/` (settings in a keyfile, not dconf), so testing never touches your real Kinora. To give it an account, use *Connect Account* in the dev build, or put `account-cookies-command` in `.dev/home/config/glib-2.0/settings/keyfile`.
- **Inspect live**: `GTK_DEBUG=interactive dev/run.sh` (or Ctrl+Shift+D) opens the GTK Inspector: widget tree, properties, live CSS.
- **Test hooks** (`dev-hooks` feature, never in release builds) drive the app without clicking:
  `KH_PAGE=feed`, `KH_SEARCH=lofi`, `KH_OPEN=0` (Nth Home video), `KH_ACTION=settings@5,connect-account@20`, `KH_NEXT_AFTER=15`, `KH_SCROLL_AFTER=12`, `KH_HOME_SCROLL=600@12` (scroll Home down 600 px after 12 s).
  The player sheet is driven by actions: `KH_ACTION=minimize-player@12,expand-player@18,player-next@22,close-player@30` (also `toggle-player`, `player-previous`); `KH_SHEET_PROGRESS=0.6@12` holds it between minimized (0) and expanded (1), `KH_MINI_DOCK=left` docks the mini-player on the left.
- **Side by side**: development builds do not hand over to an instance that is already running, so builds from several checkouts (or two windows of one, to compare states) run next to each other.
- **Design**: the look is described in [docs/design.md](docs/design.md); new interface parts use its tokens and components.
- **Faster builds**: `dev/build.sh` links with `mold` when it's installed, and uses `sccache` in linked git worktrees so parallel checkouts share compiled dependencies (in the main checkout sccache would add ~1 s to every small rebuild). Measured: a one-file Rust change rebuilds in about 3 s.
