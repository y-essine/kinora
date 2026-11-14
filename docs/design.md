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
| **DJ panel** | `dj.rs` | What plays and the queue, in the Up next column while the DJ is on. See *DJ*. |
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

### States

| State | What you see |
|---|---|
| **Closed** | Nothing. |
| **Expanded** | The video page (stage, title, description, Up next) over everything but the sidebar. |
| **Minimized** | The mini-player. Wide windows: a card in a bottom corner (336 px: the live video, under it title, channel, play/pause, next, close). Narrow windows: a bar across the bottom, above the tab bar, with the video on its left. A 3 px line in the accent shows how far the video is. The card carries the video's ambient colours, like the stage. |
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

"Previous" is the video played before in this sitting. "Next" is the first of
Up next.

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

**The DJ is a mode of the player, not another place.** Music plays in the same
player sheet as everything else: every track is an ordinary video page, with
its video on the stage. What changes is what comes next. The DJ's queue stands
where Up next would, and one amber word, *DJ on*, says who is choosing.

### Ways in and out

| Where | What | Starts from |
|---|---|---|
| Video page, action row | **Start DJ** (YouTube videos, while the DJ extension is on) | this video's radio, or the mix, following the DJ's *Start From* setting. The video plays on, untouched |
| Home, under the banner | **Your mix, endlessly** strip with *Start DJ* | the account's mix |
| Home, *Mixes* and *Your playlists* shelves | a playlist card | that playlist or mix, from its first item |
| DJ panel | **Leave** | back to ordinary playing: the video plays on, Up next and autoplay return |

Opening any other video, or closing the player, also leaves the DJ. Starting
it never interrupts what plays unless something else was asked for (the mix, a
playlist).

### The panel

In the Up next column (beside the details in wide windows, under them in
narrow ones). One panel moves from page to page with the music.

- **Card** (`.k-panel`): the kicker *DJ on* with an amber dot, a faint note
  for where the music comes from (*Picked for you* with an account, *From
  YouTube* when YouTube Music's radio failed and the plain mix stands in), and
  *Leave*. Under it what plays: 64 px artwork, title (15/600), artists,
  "album · year" for songs, and Skip.
- **Queue**: "Up next" with the number queued, then up to 30 rows: 40 px
  artwork, title, artists, length. Click a row to play it now. *Play next* and
  *Remove* appear on a row with the pointer or the focus (opacity only). Drag
  a row onto another to move it there; the target shows an amber line.

In the mini-player a small amber **DJ** tag sits before the artist, and its
Next button skips the track. Shift+N skips too; there is no "previous" in a DJ
session.

### Sound and motion

- The only motion the DJ adds is in the sound. A track **crossfades** into the
  next over the *Crossfade* setting (0 to 12 s, a slider in Preferences →
  Extensions; short tracks fade shorter, live and very short ones cut). The
  curve is equal power, so the loudness does not dip halfway.
- The picture **cuts**: the next track's page takes the stage about 1.5 s
  before the fade is due (its artwork and a spinner while it starts), and the
  old track plays on unseen until it has faded out. No visual crossfade.
- **Skip** does not wait: the old track fades out in under half a second and
  the next one starts at full volume as soon as it can.
- The next track is prepared while the current one plays, so it starts in
  about a second. If it does not (a prepared stream can fail and fall back to
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

- **Playlists play**: the shelves and the card are there; the engine's
  playlist seed and the feeds that fill the shelves arrive with kinora-core.
- **Previous track** and a visible history.
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

- Only `opacity` and `transform` are animated, plus background colour on hover.
- Durations 150–250 ms with `--k-ease`; nothing loops (the loading spinner is
  the one exception, and only while something loads).
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
- videos are no longer prepared when the pointer rests on a card.

Anything new that is decorative must check this setting (in CSS:
`window.reduce-effects …`; in Rust: `settings.boolean("reduce-effects")`).

## Budgets

Idle CPU about 0 %, Home visible within about 2 s, memory under about 150 MB.
Measure with the development build (`dev/run.sh`); note that it is an
unoptimised build.
