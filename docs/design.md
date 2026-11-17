# Kinora design guide

Kinora should feel like a small cinema: dark, calm, and about the pictures.
This guide is short on purpose. The tokens live in
[`data/resources/style.css`](../data/resources/style.css) (`:root`), the
components in `src/gui/` and `data/resources/ui/`.

## Principles

1. **Pictures first.** Thumbnails are large; text is two lines at most; every
   control that is not needed right now is hidden or dimmed.
2. **One stage, one light.** The interface is always dark (near-black, slightly
   cool). There is exactly one accent colour, a warm amber, like a projector
   lamp. Colour otherwise comes from the videos themselves.
3. **Calm motion.** Things fade or grow slightly, in 150–250 ms, and then stop.
   Nothing moves while nothing changes.
4. **Light on the machine.** Idle CPU is 0 %. No blur is ever computed on
   screen. Lists only build what is visible.
5. **Works at half a small screen.** Every page is designed at 360–680 px
   first; wide windows get more columns, not different pages.
6. **One look.** Rows, cards and panels are components. Whatever shows a
   video, also from an extension, uses them.

## Tokens

All tokens are CSS custom properties prefixed `--k-`. Components use tokens,
not raw values. libadwaita's own variables (`--window-bg-color`,
`--accent-bg-color`, …) are mapped to them, so stock widgets match.

| Group | Tokens |
|---|---|
| Surfaces | `--k-bg` `#0b0c0f` (page) · `--k-surface-1` `#121419` (panels, sidebar) · `--k-surface-2` `#1a1d24` (cards, dialogs) · `--k-surface-3` `#252a33` (popovers) · `--k-line` (hairlines) |
| Text | `--k-text` `#f3f4f6` · `--k-text-dim` (70 %, secondary) · `--k-text-faint` (50 %, hints only) |
| Accent | `--k-accent` `#ffb454` with `--k-on-accent` `#1d1200` on top of it |
| Over pictures | `--k-scrim` (badges), `--k-glass` / `--k-glass-hover` (buttons on pictures) |
| Spacing | `--k-space-1…7` = 4, 8, 12, 16, 24, 32, 48 px · `--k-gutter` = 24 px (16 px in narrow windows) |
| Radii | `--k-radius-s` 8 · `--k-radius-m` 12 (thumbnails) · `--k-radius-l` 16 (player, panels, cells) · `--k-radius-pill` |
| Shadows | `--k-shadow-1` (small floating things) · `--k-shadow-2` (the player) |
| Motion | `--k-dur-fast` 150 ms · `--k-dur` 200 ms · `--k-dur-slow` 250 ms · `--k-ease` `cubic-bezier(0.2, 0.8, 0.2, 1)` |

**Type.** The interface uses the desktop's own font (Adwaita Sans on GNOME),
untouched, so Kinora respects the user's font and size settings. The display
style (`.k-display`: 30 px, weight 800, tight tracking; Inter Display when
installed, else the same interface family) is only for the featured title and
the video title. Other steps: row title 19/700, card title 15/600, meta 13,
kicker 11/700 uppercase in the accent.

## Components

| Component | Where | Use |
|---|---|---|
| **Video card** `TFVideoItem` | `video_item.{rs,blp}` | The only way to show a video. `wide`: 16:9 poster, avatar, two-line title, "channel · age". `compact`: small poster beside the text (narrow lists, Up next). `card-width` sets the poster width. |
| **Row** `TFVideoRow` | `video_row.{rs,blp}` | A titled, horizontally scrolling shelf of cards. Takes `title`, optional `subtitle`, and any `gio::ListModel` of videos. Hides itself while empty, shows quiet placeholders while `loading`, emits `scroll-end` near its end. |
| **Featured banner** `TFFeaturedBanner` | `featured_banner.{rs,blp}` | One video, large: picture, kicker, display title, Play and Watch Later. |
| **Home** `TFHomeView` | `home_view.{rs,blp}` | The banner and a column of rows. |
| **Stage** `.k-stage` | `video_page.blp` | The player on its ambient glow. |
| **Player sheet** `TFPlayerSheet` | `player_sheet.{rs,blp}` | Holds the one video page over the window, and the mini-player it shrinks into. See *Player*. |
| **Panel** `.k-panel` | CSS | A quiet container for text (the description). |
| **DJ page** | `video_page.blp`, `dj.rs` | The player's page while the DJ is on: a record player, what plays, three large buttons, the queue. See *DJ*. |
| **Line of the track** `TFWavyBar` | `wavy_bar.rs` | A seek bar in Material 3's wavy manner: a wave as far as the track has played, a flat track with a stop dot for the rest, a thin handle between. Takes `value`, `buffered`, `playing`, `still`; tells of seeks through `connect_seek`. Only on the DJ's page so far. See *DJ*. |
| **Ripple button** `TFRippleButton` | `ripple_button.rs` | A `GtkButton` whose press ripples from the pointer, inside its rounded shape. Its look is CSS (`.k-dj-play`, `.k-dj-step`). See *DJ*. |
| **Record player** `TFTurntable` | `turntable.rs` | A deck with the track's artwork as the record's label; it turns while playing, and can be scratched. See *DJ*. |
| **Playlist card and shelf** | `playlist_shelf.rs` | A playlist or mix: a poster on a stack, with a count and a kicker. A shelf is a titled row of them. See *DJ*. |
| **Way to watch** `.k-mode-card` | `first_run.rs` | One of the four choices of the first launch. See *First run*. |
| **Badge** `.k-badge` | CSS | Small label on a picture (duration, downloaded). |
| **Buttons on pictures** `.k-glass`, `.k-play` | CSS | Secondary and primary action over an image. |

### Home

Top to bottom: the featured banner (the first recommendation), **For you**
(the rest of the account's Home feed, loading more as it is scrolled),
**Watch again** (history), **Watch Later**, **From your subscriptions**, then
one row per topic chip of the account's Home ("Music", "Gaming", …). Rows
without videos are not shown. Topic rows are fetched only when they scroll
near the screen.

### Ambient glow

The glow behind the player and behind the banner is the video's thumbnail
reduced to soft patches of its colours (`Utility::ambient_texture`): it is
scaled down to 16×9 pixels, blurred once on the CPU at 64×36, and stretched by
the GPU. Showing it costs the same as showing any small image. It is also
darkened, so text lying on it always keeps its contrast.

## Player

**The player is a place, not a page.** A video does not take you away from
what you were browsing: it rises over it, and can be put down at the bottom of
the window, still playing, while Home, Search and the rest stay fully usable.
There is one player. Whatever is opened next plays in it.

### Rules

1. **The hand leads.** Dragging moves the player exactly as far as the hand
   moved; letting go sends it to the nearer end, or where it was heading if it
   was flicked. A new gesture can take over at any moment, also mid-animation.
2. **The video never stops or jumps.** It is the same picture all the way from
   card to page to mini-player: it only changes size and place.
3. **Motion explains, and then stops.** One spring, critically damped (no
   bounce), about 0.3 s, for every move of the sheet. Nothing else animates:
   no ripples, no flourishes. With *reduce effects*, the same moves take
   0.14 s with a plain ease.
4. **It costs one transform.** The page is laid out once, at full size.
   Shrinking it only scales and moves what is already drawn (the GPU's work),
   so a transition lays nothing out again and an idle mini-player costs what a
   small video costs.
5. **The sound never clicks.** Pause fades the sound out over 30 ms and stops
   once that is heard (about 0.2 s after the press); play fades it back in.
   After a seek the sound comes in over 8 ms. Every fade is in the audio
   itself, sample by sample. (Clapper's own play button on a video pauses at
   once.)

### States

| State | What you see |
|---|---|
| **Closed** | Nothing. |
| **Expanded** | The video page (stage, title, description, Up next) over everything but the sidebar. |
| **Minimized** | The mini-player. Wide windows: a card in a bottom corner (336 px: the live video, under it title, channel, previous, play/pause, next, close). Narrow windows: a bar across the bottom, above the tab bar, with the video on its left. A 3 px line in the accent shows how far the video is. The card carries the video's ambient colours, like the stage. |
| **Fullscreen** | As before (F: screen, T: window); only from expanded. |

Opening a video grows the player out of the picture that was clicked (card or
banner), and the page fades in around it. Without such a picture on screen, it
rises from the bottom. Between expanded and minimized, the page fades first
and the bare video travels the rest of the way, taking the mini-player's
corners as it arrives.

### Gestures and keys

| Where | Do | Result |
|---|---|---|
| Expanded, on the stage | drag down | follows the hand; release to minimize or return |
| Expanded, page at its top | touchpad: two fingers down | the same |
| Expanded, on the stage | touchpad: two fingers sideways | next video (to the left), previous (to the right) |
| Expanded | Esc, I, or the chevron in the header | minimize |
| Expanded, on the video | double click on the left or right third | 10 seconds back or forward (the middle: fullscreen) |
| Expanded | Shift+N, Shift+P | next, previous video |
| Minimized | click, drag up, or touchpad two fingers up | expand |
| Minimized | drag or swipe over the nearer window edge | close |
| Minimized (card) | drag or swipe towards the middle | moves to the other bottom corner |
| Anywhere | Ctrl+I | minimize or expand |

Past its ends the sheet resists (it gives 5 % at most). Opening a channel from
the player, or picking a page in the sidebar, minimizes it: the new page is
what you asked for, the video comes along. A video opened from a card expands
the player; one started by the player itself (Up next, autoplay, next) keeps
it as it is, so a minimized player plays on as a playlist.

### Previous and Next

**Previous** and **Next** sit either side of the play button on the player
(also in fullscreen), in the mini-player, and on the DJ's page. They are one
pair of actions (`win.player-previous`, `win.player-next`; Shift+P, Shift+N;
two fingers sideways on the stage), so every button and key does the same, and
all are greyed out together when there is nowhere to go.

- **Next** is the first of Up next: what autoplay would pick. Greyed out until
  Up next has arrived.
- **Previous**, more than 3 s into a video, starts it over; within the first
  3 s it goes to the video played before in this sitting. Greyed out when
  neither applies.
- With the DJ on they move through its tracks, crossfading (see *DJ*).

The buttons are put into Clapper's own control bar and take the look of its
play button: one bar, no second row of controls. Media keys are not wired:
Clapper's MPRIS only walks its own queue, which holds one video here.

### Not built yet

- **Up next as a queue**: a sheet pulled up from the bottom of the player, with
  "Play next" and reordering. Today Up next is the column beside the details.
- **Free picture-in-picture**: the card snaps to the two bottom corners only;
  top corners and a resizable card are left out.
- **Mini-player straight to fullscreen** (F while minimized).
- **Room for the bar**: in narrow windows the mini-player covers the last
  64 px of the list behind it.

### Extensions

Extensions will contribute *data*, never widgets: a row is a title plus a list
of videos, rendered by `TFVideoRow`; a card is a `Video`, rendered by
`TFVideoItem`; settings are rendered as libadwaita preference rows. `HomeView`
already builds every row through one function (`new_row`), which is where
contributed rows will be added. This is what keeps an extension from breaking
the look.

## DJ

**The DJ is a mode of the player, not another place**, but it has a page of
its own. Music plays in the same player sheet as everything else; while the DJ
is on, the sheet shows a record player instead of the video page: no
description, no channel, no action row. One amber word, *DJ on*, says who is
choosing.

### Ways in and out

| Where | What | Starts from |
|---|---|---|
| Video page, action row | **Start DJ** (YouTube videos, while the DJ extension is on) | this video's radio, or the mix, following the DJ's *Start From* setting. The video plays on, untouched |
| Home, under the banner | **Your mix, endlessly** strip with *Start DJ* | the account's mix |
| Home, *Mixes* and *Your playlists* shelves | a playlist card | that playlist or mix, from its first item |
| DJ page, top right | **Leave** | back to ordinary playing: the track plays on as a video page, Up next and autoplay return |

Opening any other video, or closing the player, also leaves the DJ. Starting
it never interrupts what plays unless something else was asked for (the mix, a
playlist).

### The page

**One column, the same at every width**: like a phone held upright, at most
560 px wide, centred in wide windows. Large and simple, to be used from across
the room. Top to bottom:

1. **Bar**: the kicker *DJ on* with an amber dot, a faint note for where the
   music comes from (*Picked for you* with an account, *From YouTube* when
   YouTube Music's radio failed and the plain mix stands in), a small spinner
   while a track that was asked for is starting, then *Show Video*,
   *Fullscreen* and *Leave*.
2. **Record player** (`TFTurntable`): a square deck, as wide as the column
   but at most 44 % of the page's height, so the buttons and the first of
   the queue stay in view. On it a platter, a record whose label is the
   track's artwork, and a tonearm with an amber headshell. A click on it
   plays or pauses; the record can be taken by hand (see *Scratching*).
3. **What plays**: title (24/800, two lines at most), artists (16, dim),
   "album · year" for songs (13, faint). Centred.
4. **The line of the track** (`TFWavyBar`): amber, 4 px. What has played is
   a wave; then a thin upright handle; then a flat faint track, lighter as
   far as the track is loaded, ending in an amber stop dot. Elapsed time and
   length under its ends. A press or a drag anywhere on it seeks once the
   hand rests (the handle thins in the hand); the arrow keys step it.
5. **Previous, Play/Pause, Next**: Material 3's expressive buttons, 64, 96×84
   and 64 px. Previous and Next are tonal circles. The middle one says what
   plays by its shape as well as its icon: paused it is round and tonal (the
   accent at 28 %, the icon in the accent); playing it is a rounded square
   (26 px corners) filled with the accent. Under a press all three square up
   (18 px) and ripple from the pointer (`TFRippleButton`). They are the
   player's own actions (see *Previous and Next*), so they crossfade and grey
   out like everywhere else.

The line and the buttons follow the Material 3 media card of the desktop
shell (wavy progress, stop dot, shape-morphing play button), in Kinora's own
amber on its dark page.
6. **Up next**: the number queued, then up to 30 rows: 48 px artwork, title,
   artists, length. Click a row to mix into it now. *Play next* and *Remove*
   take the length's place on a row with the pointer or the focus (opacity
   only). Drag a row onto another to move it there; the target shows an
   amber line. The page scrolls; the queue peeks out under the buttons.

Behind it all lie the track's colours (the ambient glow, see above), strongest
behind the record and gone by the queue.

**The record player's motion** is the one exception to "nothing loops": it is
what the page is for, and it only runs while music plays. The line's wave is
part of it: it drifts towards the handle while music plays (16 px a second,
redrawn only when it has moved half a pixel) and flattens when paused (about
0.15 s). No frames are asked for while the line is off screen or shrunk into
the mini-player.

- While a track plays the label turns (one calm turn in 5 s; the light on
  the vinyl stays where it is) and the arm lies on the record, moving inwards
  as the track goes on: the arm is a second progress line.
- Paused, or while a track is starting, the record eases to a stop (about
  half a second) and the arm goes back to its rest.
- A new track is a new record: it drops onto the empty platter (0.35 s), and
  the arm swings on when it plays. This is the picture's part of a crossfade.
- With *reduce effects* nothing turns or swings: the arm is on the record or
  off it, and there is no glow. Frames are only drawn while something moves.

### Scratching

**The record can be taken by hand**, like the vinyl on an Instagram story:
press on it, drag it around its spindle, and it turns with the pointer while
the sound does what the record does.

- **The record follows the hand** exactly, at any speed and in both
  directions. One turn is 5 s of the track, the same as when it plays by
  itself, so holding the record at its own speed sounds like nothing happened.
- **The sound follows the record**: forwards faster or slower, backwards in
  reverse, still when the hand is still. The pitch goes with the speed, as on
  vinyl, and glides (the needle has a few milliseconds of inertia). There is
  no stutter and no click: nothing seeks while the record is held.
- **The arm, the line and the clock follow too**: they show where the needle
  is, and the track goes on from there.
- **Let go, the record runs up to speed** over about half a second, sound and
  picture together, and plays on from where it was left. A record that was
  paused comes to rest instead, and stays paused at its new place.
- It can be taken again while it runs up: that is how scratching goes.
- **A press without a drag is still play/pause**, anywhere on the deck. A drag
  that starts on the deck beside the record still pulls the page down to the
  mini-player; only the record itself is a handle.
- The first second after and twelve seconds before the needle are at hand the
  moment the record is taken; more of the track (25 s after, then 20 s
  before) arrives within a second or two. At the end of what is at hand the
  needle stops, as at the end of a record.
- Not while the DJ crossfades that track, and not with *Show Video* on (there
  is no record then).
- *Reduce effects* does not switch this off: the record only moves because a
  hand moves it.

**Fullscreen** (F, or the button in the bar) keeps the page as it is and takes
the header away. **Minimized**, the mini-player shows a band across the middle
of the record: the turning label with vinyl either side. The sheet shrinks the
page towards that band, as it does towards the video. In the mini-player a
small amber **DJ** tag sits before the artist.

**With *Show Video* on**, the track's video stands where the record player
would, as high as it would be; everything else is the same. In fullscreen it
is the video alone, with the player's own controls.

### Audio first

The DJ plays **audio only** unless *Show Video* is on (the toggle in the bar;
also in Preferences → Extensions): the record player is what an audio-only
track looks like. Audio starts in about half the time, and a crossfade does
not decode two videos. Switched to video, tracks play as videos again from the
next track on (a track that was loaded without its picture keeps its record).
Switched back, the record takes the video's place at once.

### Changing track

Every change of track the user asks for sounds the same: **Next**,
**Previous**, clicking a row of the queue, and opening a card of a song that
is in the queue all crossfade over the *Crossfade* setting, exactly like the
change at the end of a track.

- The other track's page takes the stage at once; the fade begins the moment
  it plays. Until then the current track plays on at full volume, Previous and
  Next are greyed out, and the bar shows its spinner.
- **Previous** follows the player's rule: more than 3 s in, the track starts
  over; within 3 s, the DJ goes back to the track before (which leaves the
  current one first in the queue, so Next returns to it).
- A change asked for during a fade ends that fade and starts the new one.

To make the wait disappear, tracks are **got ready ahead of time**: once a
track is under way the next one is prepared and its player loaded and held
silent at its start ("warm"); the queue row the pointer rests on for 0.4 s
gets the same. At most two players are kept warm (the next track and the
hovered one). A warm track starts in about 0.1 s.

### Sound and motion

- With **Start on the Beat** on (a switch in Preferences → Extensions → DJ,
  off by default), the next track does not come in at 0:00 but **where its
  drums come in**: the first place where the beat is there and stays, after a
  quiet intro (a pad, a guitar, a voice alone). A fade then has two beats to
  cross instead of a beat and a haze.
  - A track that gets going within its first 5 s starts at its start. So does
    one without a clear entry, and one that only gets going after a minute or
    after 30 % of its length: by then it has had a verse, and that is not an
    intro to skip. The log says which, per track.
  - It costs nothing at the fade: the next track's player waits at its entry
    instead of at its start, and still starts in about 0.1 s. A track that was
    not ready ahead of time (Previous, a pick from the queue) goes there
    first, about 0.2 s, if its entry is known from before.
  - The first track of a session plays from its start, and so does a track
    started over with Previous; the seek bar goes back before the entry like
    anywhere else.
  - It works with and without Match Beats; with both, the beats that are
    matched are those at the entry.
- With **Match Beats** on (a switch in Preferences → Extensions → DJ, off by
  default), a crossfade is also **beat-matched**: the next track plays at the
  tempo of the one that ends, at its own pitch, with its beats on the other's
  beats, so the kicks land together instead of galloping. Once the old track
  is gone, the new one eases back to its own tempo over 8 s; nothing stays
  stretched.
  - Tracks are stretched by 8 % at most. A track at about half or double
    the other's tempo is matched to every other beat. Further apart, or with
    no clear beat in either (a quiet intro, speech), the fade is the plain
    one; the log says why.
  - It costs less than a beat: the next track is held for a moment and let go
    on the beat. Next and Previous stay as quick as they are. How close the
    beats then are depends on that start (a few hundredths of a second);
    longer fades are also corrected on the way.
  - Nothing to see: it is only heard.
- A track **crossfades** into the next over the *Crossfade* setting (0 to 12 s, a slider in Preferences →
  Extensions; short tracks fade shorter, live and very short ones cut). The
  curve is equal power, so the loudness does not dip halfway, and smooth: the
  fade is in the audio, sample by sample, without steps, whatever else the
  app is busy with. The old track is let go once its silence is heard.
- The picture **changes record**: the next track's page takes the sheet about
  2 s before the fade is due (its record drops in; the arm waits at its rest
  until the track plays), and the old track plays on unseen until it has
  faded out. No visual crossfade.
- With the next track warm, its page opens 0.3 s before the fade instead. If
  it is not ready (a prepared stream can fail and fall back to
  the slow path), the current track simply plays to its end and the next one
  comes in when it is ready; nothing is cut off.

### Playlists and mixes

`playlist_shelf.rs`: a playlist is not a video, and its card says so. The
16:9 poster lies on a small stack (two sheets peeking out above it), a badge
counts its videos (mixes have no end, so no badge), and a kicker names it:
*Mix*, *Playlist*, *Album*. A play glyph appears on hover or focus. Cards are
as wide as video cards and sit on their own shelves on Home (*Mixes*, *Your
playlists*), which hide while empty.

### Not built yet

- **A visible history** (Previous walks it, nothing lists it).
- **Media keys**: see *Previous and Next*.
- **Video for the track that plays** when *Show Video* is switched on: it
  comes with the next track.
- **The queue's buttons without a pointer**: *Play next* and *Remove* need
  hover or the keyboard; a touch screen only gets "play now".
- **Scratching by touch and by keyboard**: the record is taken with a pointer
  only; two-finger scratches and a jog on the keys are not there.
- **Bars, not only beats**: Match Beats puts beat on beat, not the first beat
  of a bar on the first beat of a bar.
- **Where the old track leaves**: Start on the Beat picks the next track's
  entry; the outgoing track still fades at its end, wherever that is in its
  bars. Swung and played beats (no steady grid) only get an entry after a
  plainly quiet start.
- **Adding to the queue from elsewhere** ("Play next" on any video card): the
  engine has `play_next`/`enqueue`; no menu item uses them yet.
- **One volume during a fade**: changing the volume while two tracks cross
  only takes effect when the fade is over.

## First run

**"How do you want to watch?"** comes up once, on the first launch, over
Home. Four cards, one choice, two buttons (*Skip*, *Continue*). It can be left
with Esc, and reopened from Preferences → General → *Ways to Watch*.

A way to watch is a preset of settings that already exist. Nothing in the app
asks which one was picked, and each part can be changed afterwards.

| Way | Account extension | Home filters | DJ | Reduce effects |
|---|---|---|---|---|
| **Private**: no account; subscriptions and history stay on this computer | off | on | off | off |
| **Lite**: quick and quiet | as it is | off | off | on |
| **Connected**: the YouTube account's Home, recommendations, history | on | on | off | off |
| **Fun**: connected, with the extras | on | on | on | off |

*Connected* and *Fun* open the Connect dialog afterwards when no account is
connected. When the settings already match a way, its card is selected;
otherwise none is, and *Continue* waits for a choice. The chosen card takes the
accent as an outline and a 10 % tint: no motion.

## Motion

- Only `opacity` and `transform` are animated, plus background colour on hover
  (the exception: the corners of the DJ's three buttons, 300 ms with a slight
  overshoot, and their ripple).
- Durations 150–250 ms with `--k-ease`; nothing loops (the exceptions: the
  loading spinner, only while something loads, and the DJ's record and the
  wave of its line, only while music plays).
- Thumbnails fade in when they arrive; on hover the picture grows 4 % inside
  its frame (the card does not move, so nothing around it shifts).
- A row scrolls by one page in 250 ms when its arrows are used.

## Accessibility

- Contrast: body text 17:1 and secondary text 9:1 on the page; the accent 11:1;
  text on the accent 10:1. Text over pictures always sits on a scrim.
- Keyboard: every card is reachable with Tab and the arrow keys; focus is a
  2 px accent ring. Row arrows and banner buttons are ordinary buttons.
- Screen readers: rows are groups with a heading; cards announce "title
  uploaded by channel on date"; decorative pictures (glow, banner backdrop)
  have the presentation role.
- The system's "reduce animations" setting is respected by libadwaita's
  animations; Kinora's own switch is below.

## Lite mode (`reduce-effects`)

The `reduce-effects` setting (*Preferences → Appearance → Reduce Effects*)
adds the `reduce-effects` class to the window. With it on:

- no ambient glow (`.k-glow` is hidden);
- no transitions and no hover growth;
- rows jump instead of gliding;
- videos are no longer prepared when the pointer rests on a card;
- the DJ's record does not turn and its arm does not swing (it still turns
  in a hand: scratching is the user's own motion);
- the wave of the DJ's line does not drift (it stands while playing and lies
  flat while paused), and the DJ's buttons change shape at once and do not
  ripple. The same holds when the desktop's animations are off.

Anything new that is decorative must check this setting (in CSS:
`window.reduce-effects …`; in Rust: `settings.boolean("reduce-effects")`).

## Budgets

Idle CPU about 0 %, Home visible within about 2 s, memory under about 150 MB.
Measure with the development build (`dev/run.sh`); note that it is an
unoptimised build.
