# Kinora's architecture

Kinora has a brain and a face, and they are kept apart. The brain (`kinora-core`) knows
YouTube, the account, Home, Up next, how to get a video ready to play, and the extensions. The
face (today the GTK app) owns windows, widgets, settings storage and the video player. A second
face, like a Tauri app for Windows or an mpv-based player, can reuse the brain as it is.

```
┌──────────────────────── frontends ────────────────────────┐
│ GTK/libadwaita app (src/)        future: Tauri, mpv, CLI  │
│  widgets, GSettings, Clapper      their own UI and player  │
│  backend::Client = adapter                                 │
└──────────────┬────────────────────────────┬───────────────┘
               │ plain values in            │ implements
               │ (AccountConfig, Paths, …)  │
┌──────────────▼────────────────┐   ┌───────▼──────────────┐
│ kinora-core (crates/)         │   │ player::Player trait │
│  Core: home, up_next, watched,│   │ (in kinora-core)     │
│  prepare, fast_playback_uri   │   └──────────────────────┘
│  ExtensionRegistry + built-ins│
│  home / account / streams     │
└──────────────┬────────────────┘
               │
   pipeline-core types (pcore::Video, pcore::Channel), RustyPipe,
   InnerTube over reqwest, yt-dlp / kinora-prepare / kinora-account
```

## Layers

**`crates/kinora-core`**: a library with no gtk, gdk, glib or adw dependency.

| Module | What |
|---|---|
| `lib.rs` | `Core`, the entry point a frontend keeps in an `Arc` |
| `config.rs` | `Paths` (the cache folder), `AccountConfig` (the cookies command), `PlaybackConfig` (fast start) |
| `home.rs` | The account's YouTube Home over InnerTube: first page, chips, continuations, yt-dlp fallback, parsing |
| `account.rs` | Cookies, browser profiles (`kinora-account`), Up next and subscriptions (RustyPipe), mark watched (yt-dlp) |
| `streams.rs` | Prepared playback (`kinora-prepare`) and the experimental RustyPipe fast start (DASH/HLS) |
| `player.rs` | The `Player` trait and playback helpers |
| `extension/` | The extension model, the registry and the built-in extensions |

Rules:

- **No settings reads in core.** A frontend reads its settings at call time and passes plain
  values (`AccountConfig`, `PlaybackConfig`, the disabled list and setting values). The core
  never knows whether they came from GSettings, a JSON file or a web form.
- **Paths are passed in** (`Paths::new(cache_dir)`): `account/` holds RustyPipe's cache,
  `streams/` the prepared manifests.
- **Shared types** are pipeline-core's `pcore::Video` and `pcore::Channel` (same revision for the
  app and the core through `[workspace.dependencies]`), and the core's `HomePage`/`HomeChip`.
- **Async on Tokio.** `Core`'s futures are `Send` and need a Tokio runtime (child processes,
  HTTP). The GTK app runs them with `tspawn!` and brings results back to the GTK thread.
- **Errors** are `Box<dyn Error + Send + Sync>` messages for the user or the log.

**The GTK app** (`src/`): `backend::Client` is a GObject adapter. It reads GSettings, calls
`Core`, and turns the results into GObject stores (`home_store`, `home_chips`, `Video`,
`Channel`) with their caching and thumbnail loading. Upstream Pipeline's providers (Piped,
RustyPipe, PeerTube search, subscriptions feed) and the SQLite store are still in the app (see
*Left for later*).

## The Player trait

`kinora_core::player::Player` is what a frontend implements for its video widget:

```rust
pub trait Player {
    fn load(&self, uri: &str);
    fn play(&self);
    fn pause(&self);
    fn stop(&self);
    fn seek(&self, position: Duration);
    fn position(&self) -> Duration;
    fn duration(&self) -> Option<Duration>;     // None: unknown or live
    fn state(&self) -> PlayerState;             // Stopped | Buffering | Paused | Playing
    fn connect_event(&self, handler: EventHandler); // StateChanged, DurationChanged, EndOfStream, Error
    fn capabilities(&self) -> PlayerCapabilities { /* no gapless, no crossfade */ }
    fn queue_next(&self, uri: &str) -> Result<(), Unsupported> { Err(Unsupported) }
    fn crossfade_to(&self, uri: &str, over: Duration) -> Result<(), Unsupported> { Err(Unsupported) }
}
```

It runs on the UI thread (no `Send` bound). `play_next(player, uri, transition)` uses a
crossfade or gapless start when the player can and falls back to a cut; `reached_end` is the
"stopped within 0.75 s of the end" rule autoplay uses; `seek_by` clamps relative seeks.

The GTK video page still drives Clapper directly: it is being redesigned, and a
`ClapperPlayer(clapper::Player)` adapter is best written together with the new page.

## Extensions

An extension is an optional feature the user can switch off. It never draws UI: it describes
itself and contributes to fixed points that each frontend shows with its own widgets.

```rust
pub trait Extension: Send + Sync {
    fn manifest(&self) -> &Manifest;
    fn feed_sources(&self) -> Vec<Arc<dyn FeedSource>> { vec![] }
    fn feed_filters(&self) -> Vec<Arc<dyn FeedFilter>> { vec![] }
    fn player_hooks(&self) -> Vec<Arc<dyn PlayerHook>> { vec![] }
    fn menu_items(&self) -> Vec<MenuItem> { vec![] }
    fn activate(&self, ctx: Context, item: &str, target: MenuTarget)
        -> BoxFuture<'static, Result<ActionOutcome, Error>>;
}
```

**Manifest**: `id` (stable, `[a-z0-9-]`), `name`, `version`, `description`, `permissions`,
`settings`, `builtin`. It is serde (de)serialisable, so third-party extensions can ship it as a
file.

**Permissions**: `Network`, `Account`, `PlayerControl`. They are shown to the user, and the
`Context` an extension gets enforces them: `ctx.account()` fails without `Account` (and
extensions without it get an anonymous `AccountConfig` anyway), `choose_next`/`transition` are
only asked of `PlayerControl` extensions, and `ctx.require(Permission::Network)` guards network
use. Built-in extensions are trusted Rust code, so for them this is discipline, not a sandbox.

**Contribution points**:

| Point | Trait / type | Used by the GTK app today |
|---|---|---|
| Feed sources | `FeedSource { id, title, placement: Main \| Row, available(ctx), load(ctx, FeedRequest) -> HomePage }` | Home = first available `Main` source |
| Feed filters | `FeedFilter::filter(ctx, &mut HomePage)` | every Home page |
| Player hooks | `PlayerHook { on_watched, on_ended, choose_next, transition }` | `on_watched` (30 s into a video, or opened externally) |
| Menu items | `MenuItem { id, label, location: Main \| Home \| Video }` + `Extension::activate` → `ActionOutcome` (`Done`, `Message`, `Subscribe`, `Play`) | "Import subscriptions" runs through `activate`; the menus themselves are still hand-written |
| Settings panel | `Setting { key, title, subtitle, kind: Toggle \| Choice \| Text }` → `SettingsPanel` | Preferences → Extensions (`src/gui/extension_settings.rs`) |

**Registry**: `ExtensionRegistry` keeps extensions in registration order, rejects invalid or
duplicate manifests, and holds the disabled list and stored setting values (values that don't
fit their declared kind fall back to the default). It builds each call's `Context` (own
settings, paths, the account if permitted). `Core::new` registers the built-ins;
`Core::with_extensions` takes any registry (tests use made-up extensions).

**Persistence (GTK)**: two GSettings keys, read by `Client::load_extension_settings` at start and
on change:

- `disabled-extensions` (`as`): ids that are off; everything else is on.
- `extension-settings` (`a{sv}`): `"EXTENSION.SETTING"` → boolean (toggle) or string (choice,
  text); `extension::value_key(ext, key)` builds the key.

A Preferences page lists `client.core().extensions().settings_panels()` (title, description,
permissions, enabled, settings with current values), renders a switch per extension and a row
per setting, and writes those two keys; `registry.set_enabled(id, on)` returns the new disabled
list to store. Labels in manifests are English; translate them in the frontend.

### Built-in extensions

| id | Contributes | Settings | Off means |
|---|---|---|---|
| `account` (Network, Account) | `youtube-home` main feed source; `on_watched` marks the video watched on YouTube; menu item `import-subscriptions` (Home) | `personal-up-next`, `mark-watched` (both on) | Fully anonymous: no Home, anonymous Up next, nothing sent to the account. `Core::account_active` and the Client's `has-account` are false |
| `home-chips` | a feed filter that hides the playlist-only Mixes/Playlists chips | `hide-playlist-chips` (on) | Home is one feed without a chip bar |

What stays in the core on purpose: anonymous Up next (it works without an account; the account
extension only personalises it), prepared/fast-start playback, and connecting an account (the
browser-profile dialog is a frontend flow that ends in `AccountConfig::cookies_command`).
Autoplay ("play the first Up next video") is still in the GTK video page; with the new page it
should ask `Core::choose_next`/`Core::transition` and call `player::play_next`, which is where a
DJ extension (radio queue + crossfade) plugs in.

Experiences are sets of extensions: **Private** = `account` off; **Lite** = `home-chips` off
(later: other heavy extensions); **Connected** = all on; **Fun** = opt-in extensions like a DJ.
Extensions that should start *off* need an `enabled-extensions` list too (not added yet).

## A new frontend

A Tauri (Windows) or mpv-based frontend would:

1. Depend on `kinora-core` and create one `Core::new(Paths::new(<its cache dir>))` inside its
   Tokio runtime (Tauri already runs one).
2. Store its settings however it likes and pass them in per call: `AccountConfig`,
   `PlaybackConfig`, `extensions().set_disabled(..)`, `extensions().set_values(..)`.
3. Expose `Core` calls as commands (`home`, `home_more`, `up_next`, `prepare`,
   `prepared_uri`, `fast_playback_uri`, `watched`, `activate`) and serialise `pcore::Video` for
   the web UI.
4. Implement `Player` for its player (libmpv, or a bridge to an HTML `<video>`), and call
   `Core::watched`/`ended` from its events.
5. Render `settings_panels()` and `menu_items(location)` with its own widgets.

What it would still need from the GTK app's side today: subscriptions, watch later, history and
search (upstream Pipeline's `papi` providers plus `src/store/sqlite.rs`), and the helper
programs `yt-dlp`, `kinora-prepare` and `kinora-account` (the core calls them by name; Windows
would ship them next to the app).

## Left for later

- Move the SQLite store, the `papi` provider setup (Piped/RustyPipe/PeerTube) and import/export
  into the core. The store uses the GObject `Filter`, so it needs a plain filter type first.
- A `ClapperPlayer` implementing `Player`, and autoplay through `choose_next`/`play_next`.
- Render menus from the registry in the GTK app (settings panels already are; the account menu is still
  hand-written in `window.rs`).
- Third-party extensions: load manifests from files and run their code sandboxed (e.g. WASM),
  where permissions become a real boundary; an `enabled-extensions` list for opt-in ones.
- Home rows: `FeedPlacement::Row` exists, no frontend shows rows yet.
