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
| `beats.rs` | Tempo and beat grid of PCM a frontend hands in, tempo matching, the cache by video id (see *Beat matching*) |
| `extension/` | The extension model, the registry and the built-in extensions |

Rules:

- **No settings reads in core.** A frontend reads its settings at call time and passes plain
  values (`AccountConfig`, `PlaybackConfig`, the disabled list and setting values). The core
  never knows whether they came from GSettings, a JSON file or a web form.
- **Paths are passed in** (`Paths::new(cache_dir)`): `account/` holds RustyPipe's cache,
  `streams/` the prepared manifests, `beats/` the analysed tracks.
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
| `dj` (Network, Account, PlayerControl) | nothing to the fixed points: frontends call `Core::dj_*` (see *The DJ*) | `crossfade-seconds` (number 0..12, 6; stored as text), `avoid-repeats` (on), `show-video` (off: the DJ plays audio only), `match-beats` (off; see *Beat matching*), `start-on-beat` (off; see *Start on the Beat*), `start-from` (`current-video` \| `my-mix`) | `Core::dj_*` fail; nothing is fetched |

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
and fade both on `gains(elapsed)` for `fade`, then stop the old one (a cut: on end of stream);
call `advance()` (or `skip()`), then refill in the background. Fade in the audio itself if the
player allows it (the GTK frontend does, see *The deck's audio*): a sound device's volume set
every ~50 ms changes in steps, and every step is a small click.

### Beat matching

`kinora_core::beats` finds the tempo and the beats of a piece of audio, says how to match one
track to another, and keeps the answers on disk. It decodes nothing: a frontend hands in mono
PCM at a low rate (11 kHz is plenty) and gets a `BeatGrid { bpm, anchor, confidence }` back
(`anchor`: the time of one beat, in seconds of the track).

- **Analysis** (`beats::analyse(pcm, rate, start, near)`): an onset envelope at 200 Hz (the
  energy of three bands, compressed, its rise from one step to the next, what stands out from
  the local mean); the tempo from the envelope's autocorrelation, read with a comb (a period
  also shows at its multiples) and a broad preference for tempos around 120 BPM, then refined
  against the envelope so that the beats of the whole window stay in step (0.05 % steps); the
  phase where a pulse train collects the most onsets. `near` keeps the search within 6 % of a
  tempo already known for the track. `confidence` is how alike the envelope is to itself one
  to four beats on: a drum machine gives 0.6 and more, a rap verse over a steady beat 0.13 to
  0.35 (and the right tempo), an intro without drums under 0.1 (and any tempo);
  `BeatGrid::is_usable` draws the line at 0.12. Measured (release build): 45 s in 11 ms, 12 s
  in 3 ms; on steady tracks, 12 s windows all along the track agree within 0.1 %.
- **Half and double tempo** stay ambiguous for some music (70 BPM may come out as 140), so
  `beats::match_tempo(outgoing_bpm, outgoing_rate, incoming_bpm, max_stretch)` also tries the
  incoming track at half and double its tempo, and gives a `BeatMatch` (the rate to play it at)
  or why not (`NoMatch::TooFar`: more than `MAX_STRETCH`, 8 %). `BeatMatch::start_delay` says
  how long to hold the incoming track so that the beats land together, `BeatMatch::error` how
  far apart they are for two positions.
- **Kept on disk**: `Core::analyse_beats(video, pcm, rate, entry, length)` analyses a track's first seconds
  (`beats::INTRO`, 45 s: what a fade brings in) and writes `<cache>/beats/<video id>.json`;
  `Core::beats(video)` reads it. "No beat found" is kept too.
- `cargo run --release -p kinora-core --example beats_file -- track.raw` tries it on real
  audio (`ffmpeg -i TRACK -ac 1 -ar 11025 -f f32le track.raw`).

What the GTK frontend does with it (`src/gui/dj.rs`, only with the DJ's *Match Beats* setting
on; off, none of this runs):

1. **Ahead of time**: when the next track's stream is prepared, a thread decodes its first 45 s
   (`deck_audio::decode`: `uridecodebin3` to mono 11 kHz, read with a pad probe) and has the
   core analyse and keep them. About 1.5 s for a track never seen (1.3 to 5 s measured),
   nearly all of it the download; nothing for one that is on disk.
2. **When a fade is asked for** (also by Next): the beats of the track that ends are read from
   the last 12 s it actually played (its `Tap`, below), on a thread, in a few milliseconds. So
   they are known wherever in the track the fade starts.
3. **The start**: a warm track is not started when its page opens. It waits at its very
   start (the one place a player is at for certain), is told its tempo (`BeatMatch::rate`),
   and is let go after `start_delay`: less than a beat later, so that its beats land on the
   outgoing ones. The fade begins then. (What the sound device had already taken in, about a
   second, still plays at the track's own tempo; `start_delay` counts that in.) A track that
   was not warm starts when it can, and is then paused and let go the same way.
4. **During the fade**, the outgoing beats are read again from its `Tap` every 1.5 s, and a
   tempo that comes out different is followed. The two positions are compared every 50 ms
   (`BeatMatch::error`), but only acted on once the incoming player has played for 8 s (see
   below): from then on, beats more than 12 ms apart are nudged together with a measured dose
   of tempo (at most 4 %, about 0.6 s). A fade of the default 6 s therefore rests on its
   start alone.
5. **Afterwards** the incoming track goes back to its own tempo over 8 s (`ease_tempo_home`).

How well it does (a mix of 1990s dance tracks, `KH_DJ_PLAYLIST=RDy6120QOlsfU@3`, Next every
27 s: five of seven fades matched, with stretches from -5.2 % to +2.4 %; the other two had no
clear beat on one side; the players' positions read every 50 ms): beats 3 to 6 ms apart on
average and 12 ms at worst per fade. The start alone put them 2 to 11 ms apart, and four of
the five fades needed no nudge after it. That is with a sound device that keeps good time (a
null sink recorded at 20 ms latency) and the comparison acted on from the start
(`KH_TRUST_CLOCK=1`). On an ordinary device a player's clock wanders by up to 70 ms in its
first seconds (the click track again), which is why step 4 waits, and why the fade's own
figures say "clock settling" until then. What is heard is right all the same: in a fade
recorded under those conditions, the kicks of both tracks, folded over one beat, fall into one
peak 12 ms wide with a shoulder 20 ms beside it (the fade's own figures claimed 30 to 90 ms).
It has not been judged by ear.

### Start on the Beat

With the DJ's *Start on the Beat* on, a track is brought in where it gets going instead of at
its start. `beats::find_entry(pcm, rate, length)` looks at the track's first 76 s
(`beats::ENTRY_WINDOW`) and gives an `Entry { start, grid, found }`:

1. The **drive** of a moment: the mean of an onset envelope over the 4 s after it, read every
   0.5 s. The envelope is the one of the beat analysis with other weights (kick 1.5, middle
   0.5, hats 1): drums are in the low and the high band, voices, pads and guitars in the middle.
2. **Going**: 70 % of the drive of the strongest tenth of what was seen. The first moment at
   which the drive is there, stays above three quarters of it for 8 s and has it on average is
   the rough entry, good to a second or two.
3. Sooner than 5 s: `FromTheStart`. Later than 60 s or 30 % of the track (`ENTRY_SEARCH`,
   `ENTRY_SHARE`): `TooFar`. Both start at 0.
4. **The beat**: the 16 s from there are analysed like an intro (`analyse`), which gives the
   grid of that part; it has to be clear (confidence 0.2). The entry is the beat, from four
   bars before the rough moment to two after, where the onsets on four beats exceed those on
   the four before by the most (and which is itself a strong one). `start` is 30 ms before it,
   `grid` is anchored on it. Where a part begins is a phrase's first beat, so no bars are
   counted; the number of beats since the track's first sound is given when it is a whole
   one (`Later { beats_in }`: 16, 48, 64 on most of the dance tracks tried, but 49, 63 and 75
   too, so nothing is moved by it).
5. **Without a steady beat** (swung, played by hand): only after a plainly quiet start (the
   drive of the 4 s before at most 45 % of "going"), the first onset half as strong as the
   strongest of the next seconds; no grid. Else `NoClearBeat`, and the track starts at 0.

It is kept with the beats (`TrackBeats::entry`; the cache's version is 2, older files are
analysed again). `Core::analyse_beats(video, pcm, rate, entry, length)` does both;
`TrackBeats::grid_at(start)` gives the beats a fade goes by: the entry's when the track starts
there, else the intro's.

In the GTK frontend (`src/gui/dj.rs`):

- **Ahead of time**: with the setting on, the thread that analyses a prepared track decodes
  76 s instead of 45 s (1.5 to 2.3 s measured) and looks for the entry (9 ms in a release
  build, 0.6 s in a debug build).
- **Warm at the entry**: a warm player is sent to the entry (`VideoPage::seek_entry`, an
  accurate seek, paused and silent) and counts as ready when it is there, 2.4 to 3.2 s after
  warming began for a track never seen, 1 s for one on disk. If the entry is not worked out
  within 12 s the track is ready at its start. From then on everything is as at 0: it starts
  in 0.1 s, or held for the beat with *Match Beats* (the hold counts from wherever the player
  rests).
- **Not warm** (Previous, a pick from the queue, Next before the next track was ready): if the
  entry is on disk, the incoming player is sent there once it plays, silent as it still is,
  and the fade starts when it is there (0.2 s measured; given up after 2.5 s). If it is not
  on disk, the track starts at its start.
- A cut (the crossfade off, or a track that ended with nothing to fade to) uses the warm
  player as it is, so it starts at the entry too. The first track of a session is never
  warm, and starts at 0. "Previous" counts its 3 s from the entry.

Measured on real tracks (`beats_file`, and the low band's level 8 s before and after the
entry): Wake Me Up 39.6 s (+12 dB below 180 Hz), Animals 23.2 s (+20 dB), World, Hold On
16.9 s (+22 dB), Lean On 29.6 s (+14 dB), In My Mind 47.3 s (+13 dB), Meet Her at the
Loveparade 21.5 s (+17 dB), Zombie 12.7 s (+26 dB, no grid), Sunset Lover 42.2 s (+7 dB),
Ecuador 34.2 s (+10 dB), Calabria 2008 38.2 s (+5 dB); Better Off Alone, Feel the Beat and
Freestyler from their start. Less sure: Sandstorm 23.7 s (the riff with its snare roll, the
kick is later), Children 32.8 or 42.5 s and Played-A-Live 18.0 or 11.6 s depending on the
stream that was decoded (their drive builds up slowly, and the threshold falls differently).
Missed: Tom's Diner (the beat comes in in two steps and is swung; it starts at 0). With
*Match Beats*, on a null sink: the beats 0, 0, 3, 13 and 38 ms apart at the first reading of
five fades that started at an entry, 4 to 8 ms on average over each fade. Not judged by ear.

Looked at and left out (others' work, ideas only, no code): Mixxx's AutoDJ only skips digital
silence (-60 dB) and takes intro and outro cues set by hand; Vande Veire and De Bie's
dnb-autodj labels a segment "high" when its loudness and its onsets are above the track's
mean and cues a track in at a low-to-high boundary, and Zehren et al. want the part after a
cue to be salient (0.4 of the track's maximum): steps 1 and 2 are that idea on the envelope
that was there; their bar and phrase counting needs a downbeat model. Two sources per video
were tried as hints: YouTube's "most replayed" curve peaks at the drop or the chorus (86 to
92 s in Animals, 186 s in Sandstorm), past where an entry may be, and costs a page per
track; SponsorBlock's `music_offtopic` at a video's start would have changed one of 23
tracks tried (18 s of film before Feel the Beat), its `poi_highlight` is a favourite moment
with a vote or two, and it would send every track's id (or its hash's start) to one more
service. Neither is used.

### The deck's audio

Scratching and beat matching need three things Clapper's player does not offer, so the GTK
frontend adds them around it (`src/gui/deck_audio.rs`, `src/gui/scratch.rs`). Only the
`gstreamer` crate is used; audio is read and written with pad probes and `appsrc`'s signals, so
no further GStreamer library is linked (the Windows build needs nothing new; without the
`scaletempo` element there is simply no beat matching).

What was measured first, on a prepared HLS audio stream (GStreamer 1.28, Clapper 0.10):

- Clapper's `speed` is a flushing seek: about 0.15 s without sound each time, and it changes
  the pitch (no `scaletempo` in its pipeline by default). Backwards (a negative speed) works
  but takes about a second to start.
- A seek costs 0.15 to 0.4 s of silence. Scratching by seeking is out of the question.
- A GStreamer "instant rate change" keeps playing but is not taken up by `scaletempo`, and it
  breaks the position the player reports on these streams.
- The sound device has taken in about 1.2 s more than is heard. A second of that is playbin's
  queue after the audio filter (`aqueue`, at most 1 s), the rest the sound device's buffer and
  the sound server's (measured with the `dev-latency` test hook).
- A player's clock (the sound server's, for its stream) wanders in the first seconds it
  plays: by up to 3 % in rate and 70 ms in place, settling within about 8 s. A null sink
  recorded at a short latency (`parec --latency-msec=20`) does not show this, an ordinary
  device and a null sink recorded any other way do. To measure a fade's timing, play to such
  a null sink (`PULSE_SINK=…`) and set `KH_TRUST_CLOCK=1`, which has the DJ act on the
  positions from the start.

So:

- **`Tap`**, each page's audio filter (`audioconvert ! capsfilter ! identity ! scaletempo`):
  - *keeps the last 12 s* the player decoded (a DJ page only), with their place in the track:
    the scratch's audio, there the moment the record is taken, and the outgoing track's beats;
  - *changes the tempo without a seek*: a probe pushes a new segment event to `scaletempo`
    between two buffers, with the same place in the track and on the clock and another rate.
    The tempo changes there, at the same pitch, without a gap, and the player's position
    stays true: checked with a click track recorded off a null sink, where every click was
    heard within 5 ms of when the reported position passed it, through tempo changes of up to
    4 % a second apart. Segments that come from the player (after a seek) are rewritten on
    the way. A tempo set now is heard about 1.2 s later; `Tap::flow` says from where in the
    track, which is what the DJ's timing goes by;
  - *stays engaged* for a track whose tempo will be changed (`Tap::set_engaged`, said before
    the player starts; the DJ's pages with *Match Beats* on): its own tempo is then 1.00002
    instead of 1. At exactly 1 `scaletempo` only passes audio through, and going between
    that and any other rate loses about 50 ms of audio, which the sound device puts right a
    second later with a gap (the same recording showed both). Between two other rates
    nothing is lost. A tap that is not engaged leaves the audio untouched;
  - *fades the player's sound* (`Tap::fade_to`, `VideoPage::fade_gain` and `set_fade_gain`):
    a gain applied to the samples where the tap's audio leaves playbin's queue (a probe on the
    queue's output), so a change is heard in about 0.2 s instead of 1.2 s. A fade moves with the
    audio, sample by sample, on an equal-power curve (the DJ's crossfade) or a raised cosine
    (short ones); the main loop only starts it. Before, the DJ set both players' volumes every
    50 ms: each step was a click (about five per 6 s crossfade on a steady recording), and a main
    loop held up by a new page (100 to 500 ms in a release build) made the volume jump by as
    much. `Tap::is_silent` says when a fade to silence has been heard (the heard position has
    passed it), so the old player of a fade is paused and let go only then. After every flush
    (a seek, an entry, a hand back) the sound comes in over 8 ms instead of with a click. A
    pause asked for by the user (`VideoPage::play_pause`) fades out over 30 ms and pauses once
    that is heard, and play fades back in; Clapper's own buttons on a video pause at once;
  - *says where the player really is* (`Tap::position`), to a few milliseconds: the
    pipeline's clock read through the segments the tap sent. Clapper's own `position` is
    refreshed a few times a second, and the position the sound device reports goes by the
    newest segment, whose audio is not heard yet: around every change of tempo it is off by
    up to 50 ms for more than a second. Stretched audio is also heard a little off where its
    segment puts it (30 ms late at 0.94, 30 ms early at 1.06), which is counted in.
- **`decode`**: a stretch of a prepared stream as PCM, by a pipeline of its own on the caller's
  thread.
- **`Scratch`**: while the record is held, the track's player is paused and this plays instead:
  the PCM around the needle, read with cubic interpolation at the hand's speed, forwards or
  backwards, through `appsrc ! audioconvert ! audioresample ! autoaudiosink` in 5 ms buffers
  (the device is asked for a 40 ms buffer). The needle heads for where the hand is, at the
  hand's speed, with a few milliseconds of inertia, so the pitch glides instead of stepping
  with the pointer's events; where it stands still it is silent; read faster than recorded,
  the sound is dulled first. Its audio is the `Tap`'s 12 s, with 25 s after and 20 s before
  decoded onto it in the background.
- **Handing back** (`video_page.rs`, `scratch_step`): let go, the needle runs up to the
  recorded speed in the scratch itself. The player is told to seek (accurately, paused) to
  where the needle will be once it is nearly at speed, 0.8 s on or more; when the needle gets
  there the player is told to play, and it comes in over its first 64 ms of audio (its tap's
  gain, set before it is told to play, and silent from the seek on) while the scratch goes out. A player that is slow to start is met further on instead. No seek is ever heard.
- The scratch's sound device gets its volume said outright (the player's): a sound server
  gives a new stream the volume it remembers for the app, whatever that was.
- Measured (the scripted scratch of the `dj-scratch` test hook, recorded off a null sink): 25 ms of
  silence when the record is taken, none when it is handed back, the player within 20 ms of
  the needle then; more audio 1.5 s after the record is taken. Memory: 4 MB for the 12 s a
  DJ page keeps, 20 MB more while a record is in a hand.

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
