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
  The player sheet is driven by actions: `KH_ACTION=minimize-player@12,expand-player@18,player-next@22,close-player@30` (also `toggle-player`, `player-previous`); `KH_SHEET_PROGRESS=0.6@12` holds it between minimized (0) and expanded (1), `KH_MINI_DOCK=left` docks the mini-player on the left. `video.NAME` actions go to the open video page (e.g. `KH_ACTION=video.play-pause@22` pauses, which shows the controls). `KH_PREFS_PAGE=extensions` with `KH_ACTION=settings@3` opens Preferences on that page. The seek bar's buffered part logs with `RUST_LOG=kinora::gui::buffer_bar=debug`; a window on a hidden workspace never sets the seek bar's range, so it shows only on screen.
  Previous and Next are `player-previous` and `player-next` everywhere (the buttons, the keys and the hooks are one path). With the DJ on they crossfade, and the log shows it: `DJ: next by hand: crossfade to … over the 6 s setting`, `DJ: started after 0.1 s (warm|warming|prepared|cold); fading over 6.0 s`, then `DJ: fade 0.62/6.00 s  out 0.99  in 0.16` twice a second. `dj-back` goes back one track wherever the current one is (Previous restarts it when more than 3 s in), `dj-queue-play-now` mixes into the queue's third track, `dj-queue-hover` rests the pointer on it (prepare and warm: `DJ: warm … ready in 0.8 s`), `dj-show-video` switches between audio only and video. `video.play-pause` pauses, which also shows the player's controls for a screenshot.
  The DJ: `KH_ACTION=dj-start@12` (from the open video; use with `KH_OPEN=0`), `dj-start-mix@5` (from the mix, no video needed), `dj-leave`, `dj-log` (what the session holds, to the log), `dj-near-end` (jump to 14 s before the end, to watch a crossfade: the log shows both gains twice a second), `dj-show-queue` (scroll the page to the queue), `dj-queue-play-next` / `dj-queue-remove` / `dj-queue-play-now` (the queue's buttons, on its third track). `KH_PLAYLIST_DEMO=1` fills Home's Mixes and playlists shelves with samples.
  `KH_DJ_PLAYLIST=ID@4` has the DJ play a playlist, album or mix (`LL` = Liked videos, `RD…` mixes, `PL…`).
  `KH_FIRST_RUN=1` shows "How do you want to watch?". While any `KH_` hook is set it does not come up by itself, so test runs are never interrupted.
  A window on a hidden workspace is not laid out: scroll hooks only work after the window was shown once (take a screenshot first).
- **Side by side**: development builds do not hand over to an instance that is already running, so builds from several checkouts (or two windows of one, to compare states) run next to each other.
- **Design**: the look is described in [docs/design.md](docs/design.md); new interface parts use its tokens and components.
- **Faster builds**: `dev/build.sh` links with `mold` when it's installed, and uses `sccache` in linked git worktrees so parallel checkouts share compiled dependencies (in the main checkout sccache would add ~1 s to every small rebuild). Measured: a one-file Rust change rebuilds in about 3 s.
