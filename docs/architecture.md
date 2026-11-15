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
| `home.rs` | The account's YouTube Home over InnerTube: first page, chips, continuations, yt-dlp fallback, parsing (videos, and playlist/mix cards apart) |
| `playlist.rs` | `PlaylistCard` / `PlaylistKind` (Mix, Playlist, Album), playlist id clean-up, the lockup parser (see *Playlists and mixes*) |
| `account.rs` | Cookies, browser profiles (`kinora-account`), Up next and subscriptions (RustyPipe), mark watched (yt-dlp) |
| `streams.rs` | Prepared playback (`kinora-prepare`) and the experimental RustyPipe fast start (DASH/HLS) |
| `player.rs` | The `Player` trait and playback helpers |
| `dj/` | The DJ engine: YouTube Music radio, the session queue, crossfade planning (see *The DJ*) |
| `extension/` | The extension model, the registry and the built-in extensions |

Rules:

- **No settings reads in core.** A frontend reads its settings at call time and passes plain
  values (`AccountConfig`, `PlaybackConfig`, the disabled list and setting values). The core
  never knows whether they came from GSettings, a JSON file or a web form.
- **Paths are passed in** (`Paths::new(cache_dir)`): `account/` holds RustyPipe's cache,
  `streams/` the prepared manifests.
- **Shared types** are pipeline-core's `pcore::Video` and `pcore::Channel` (same revision for the
  app and the core through `[workspace.dependencies]`), and the core's `HomePage`/`HomeChip`/
  `PlaylistCard`.
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
| Settings panel | `Setting { key, title, subtitle, kind: Toggle \| Choice \| Number \| Text }` → `SettingsPanel` | Preferences → Extensions (`src/gui/extension_settings.rs`) |

**Registry**: `ExtensionRegistry` keeps extensions in registration order, rejects invalid or
duplicate manifests, and holds the disabled list and stored setting values (values that don't
fit their declared kind fall back to the default). It builds each call's `Context` (own
settings, paths, the account if permitted). `Core::new` registers the built-ins;
`Core::with_extensions` takes any registry (tests use made-up extensions).

**Persistence (GTK)**: two GSettings keys, read by `Client::load_extension_settings` at start and
on change:

- `disabled-extensions` (`as`): ids that are off; everything else is on.
- `extension-settings` (`a{sv}`): `"EXTENSION.SETTING"` → boolean (toggle) or string (choice,
  number in decimal, text); `extension::value_key(ext, key)` builds the key.

A Preferences page lists `client.core().extensions().settings_panels()` (title, description,
permissions, enabled, settings with current values), renders a switch per extension and a row
per setting, and writes those two keys; `registry.set_enabled(id, on)` returns the new disabled
list to store. Labels in manifests are English; translate them in the frontend.

### Built-in extensions

| id | Contributes | Settings | Off means |
|---|---|---|---|
| `account` (Network, Account) | `youtube-home` main feed source; `on_watched` marks the video watched on YouTube; menu item `import-subscriptions` (Home) | `personal-up-next`, `mark-watched` (both on) | Fully anonymous: no Home, anonymous Up next, nothing sent to the account. `Core::account_active` and the Client's `has-account` are false |
| `home-chips` | a feed filter that, when asked, hides the playlist-only Mixes/Playlists chips and the playlist cards | `hide-playlist-chips` ("Hide Mixes and Playlists", off: they are kept so the DJ can play them) | Home is one feed without a chip bar |
| `dj` (Network, Account, PlayerControl) | nothing to the fixed points: frontends call `Core::dj_*` (see *The DJ*) | `crossfade-seconds` (number 0..12, 6; stored as text), `avoid-repeats` (on), `show-video` (off: the DJ plays audio only), `start-from` (`current-video` \| `my-mix`) | `Core::dj_*` fail; nothing is fetched |

What stays in the core on purpose: anonymous Up next (it works without an account; the account
extension only personalises it), prepared/fast-start playback, and connecting an account (the
browser-profile dialog is a frontend flow that ends in `AccountConfig::cookies_command`).
Autoplay ("play the first Up next video") is still in the GTK video page; with the new page it
should ask `Core::choose_next`/`Core::transition` and call `player::play_next`. The DJ does not
take autoplay over: a DJ session is started on purpose and has its own queue.

Experiences are sets of extensions: **Private** = `account` off; **Lite** = `home-chips` off
(later: other heavy extensions); **Connected** = all on; **Fun** = opt-in extensions like a DJ.
Extensions that should start *off* need an `enabled-extensions` list too (not added yet).
The GTK app offers the four as presets on its first launch (`src/gui/first_run.rs`; the table is
in docs/design.md, "First run"): they write `disabled-extensions` and `reduce-effects`.

## The DJ

A continuous music session from a seed: a video (its YouTube Music radio), "my mix" (`RDMM`,
personal when the account is active, YouTube's popular mix otherwise), or a playlist, album or
mix (`DjSeed::Playlist`, see *Playlists and mixes*). The engine is in `kinora_core::dj`, with no
I/O except `dj::radio::fetch`; the frontend draws the session and plays the audio.

- **Radio** (`dj::radio`): InnerTube `next` on music.youtube.com (`WEB_REMIX`, playlist
  `RDAMVM<videoId>`), signed in with the same cookies.txt → `Cookie` + `SAPISIDHASH` auth as Home
  (`youtube::Auth`, origin `https://music.youtube.com`) when `Core::account_active`, else
  anonymous. About 50 tracks a page plus a radio continuation token. Fallbacks, in order: the
  same request anonymously, then regular YouTube's mix (`WEB`, `RD<videoId>`, 25 videos a page,
  continued by asking for the mix from its last video). Parsed into `dj::Track` (id, title,
  artists with channel ids, album and year for songs, duration, thumbnail, kind
  Song/MusicVideo/Video). `Track::to_video()` gives the `pcore::Video` to prepare and play.
- **Session** (`DjSession`): `current`, `upcoming`, `history` (with `skipped`); `advance`, `skip`, `back` (the track before becomes current again, the current one waits first in the queue),
  `play_next`, `enqueue`, `remove`, `move_to`. Dedupe keeps out the same video and the same song
  by the same main artist in another upload (`dj::track_key`: brackets and " - " version notes
  like "(Official Video)", "(Lyrics)", "(2022 Remaster)", "Live" dropped; remixes, covers and
  acoustic versions kept apart). With `avoid-repeats`, nothing played comes back; removed tracks
  never do. `refill_request()` says what to fetch when fewer than 10 are left: the next page, or a
  new radio from the most recent track whose radio was not used yet (radios that only repeat
  themselves are dropped after two pages).
- **Transitions** (`dj::plan_transition`, pure): `TransitionPlan { start_at, fade }` from the
  current track's length and position, the next one's length and the crossfade setting. The fade is
  at most 1/8 of the shorter track, none under 30 s or for unknown lengths (live); planned late,
  it fades over what is left. `plan.gains(elapsed)` / `plan.curve(steps)` give equal-power volumes
  (`cos`/`sin`, linear amplitude 0..1); `plan.transition()` is the `player::Transition` for players
  that crossfade themselves.

`Core`: `dj_settings()`, `dj_seed(account, open_video)`, `dj_start(account, seed) -> DjSession`,
`dj_more(account, &RadioRequest) -> RadioPage` (no session borrowed across the request; feed the
page to `session.extend(&request, page)`, or `session.fetch_failed(&request)`),
`dj_refill(account, &mut session)`, `dj_transition(duration, position, &next) -> TransitionPlan`.
All fail while the extension is off. `cargo run -p kinora-core --example dj_radio -- [VIDEO_ID |
mymix]` and `--example dj_playlist -- [--shuffle] [PLAYLIST_ID_OR_URL ...]` try it against the
live API; the parser tests use recorded anonymous responses in
`crates/kinora-core/src/dj/fixtures/` (and `src/fixtures/` for Home's lockups), cut down to what
the parsers read, without tracking data, visitor data or playback URLs.

What a frontend does: keep the `DjSession`; when a track starts, prepare `peek_next()` and plan
the transition with the player's real duration; at `start_at` start the next track at volume 0
and set both volumes from `gains(elapsed)` every ~50 ms for `fade`, then stop the old one (a cut:
on end of stream); call `advance()` (or `skip()`), then refill in the background.

### Playlists and mixes

Home and the account list playlists as `PlaylistCard { id, title, kind: Mix | Playlist | Album,
thumbnail, owner, owner_id, video_count, first_video_id }` (`kinora_core::playlist`). The kind
comes from the id (`RD…` mix except `RDCLAK…`, `OLAK5uy_…` album, anything else a playlist;
`LL`/`LM`/`WL` are liked videos, liked music, watch later) and the "Mix" badge. `normalize_id`
takes an id, a `VL…` browse id or a URL with `list=`.

- **Home**: `HomePage::playlists` holds the page's playlist and mix lockups, apart from
  `videos` (which are unchanged). `Core::home`/`home_more` still skip pages without videos (the
  GTK list); `Core::home_with_playlists`/`home_more_with_playlists` count a page of playlists as a
  page, so the "Mixes" and "Playlists" chips' feeds come back after one request
  (`FeedRequest::playlists` tells the feed source). The yt-dlp fallback lists playlists too.
- **Account**: `Core::account_playlists(account)` (active account only): YouTube's Playlists page
  through RustyPipe's `saved_playlists` (`FEplaylist_aggregation`, up to 500), with Liked videos
  and Watch later added when missing. Not yet tried against a live account.
- **DJ**: `DjSeed::Playlist { id, title, shuffle }`, made with `DjSeed::playlist(id, title)` (cleans
  the id; `.shuffled(true)`) or `card.dj_seed(shuffle)`. `dj_start` asks YouTube Music's `next`
  for the playlist id: 50 tracks a page in the playlist's order, continued with a token
  (`shuffle` sends YouTube Music's shuffle parameter, which shuffles the whole playlist). The
  fallback is regular YouTube's playlist panel (200 a page, then a token; signed in first, so
  private playlists can play; shuffled page by page). Nothing is current at the start:
  `advance()` for the first track. When the playlist has no more pages, `refill_request()` starts
  the radio of its last queued track, and the session goes on as a radio. A mix (`RD…`) is
  endless like a radio and never shuffled. `session.in_playlist()` (no radio started yet) and
  `session.is_from_playlist(&track)` let a frontend mark where the playlist ends in the queue.
  Measured anonymously: a 248-track playlist starts in ~0.4 s once connected (the very first
  request of a process takes 3-4 s), each further page ~0.5 s.

## A new frontend

A Tauri (Windows) or mpv-based frontend would:

1. Depend on `kinora-core` and create one `Core::new(Paths::new(<its cache dir>))` inside its
   Tokio runtime (Tauri already runs one).
2. Store its settings however it likes and pass them in per call: `AccountConfig`,
   `PlaybackConfig`, `extensions().set_disabled(..)`, `extensions().set_values(..)`.
3. Expose `Core` calls as commands (`home`, `home_more` or their `_with_playlists` versions,
   `account_playlists`, `up_next`, `prepare`, `prepared_uri`, `fast_playback_uri`, `watched`,
   `activate`, the `dj_*` calls) and serialise `pcore::Video` (and the serde types
   `PlaylistCard`, `DjSeed`, `DjSession`) for the web UI.
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
- The DJ is on by default but idle until started; for the Fun experience it could move to the
  opt-in list. (Its UI is `src/gui/dj.rs`: the session on the UI thread, two video pages
  crossfaded by volume. It drives Clapper directly rather than through `Player::crossfade_to`,
  because a track there is a whole page with its own player and stream fallbacks.)
- Render menus from the registry in the GTK app (settings panels already are; the account menu is still
  hand-written in `window.rs`).
- Third-party extensions: load manifests from files and run their code sandboxed (e.g. WASM),
  where permissions become a real boundary; an `enabled-extensions` list for opt-in ones.
- Home rows: `FeedPlacement::Row` exists, no frontend shows rows yet.
