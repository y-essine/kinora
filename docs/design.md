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
| **Panel** `.k-panel` | CSS | A quiet container for text (the description). |
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

### Extensions

Extensions will contribute *data*, never widgets: a row is a title plus a list
of videos, rendered by `TFVideoRow`; a card is a `Video`, rendered by
`TFVideoItem`; settings are rendered as libadwaita preference rows. `HomeView`
already builds every row through one function (`new_row`), which is where
contributed rows will be added. This is what keeps an extension from breaking
the look.

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
