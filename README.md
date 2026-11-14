<div align="center">
 <h1>🏠 Pipeline Home</h1>
 <strong>Pipeline, with your YouTube account's Home feed and Up next</strong>
</div>
<br>

Pipeline Home is a spin-off of [Pipeline](https://gitlab.com/schmiddi-on-mobile/pipeline) by Julian
Schmidhuber and contributors, a GTK app for watching YouTube and PeerTube without the website. It keeps
everything Pipeline does and adds the parts of YouTube that need an account:

- **Home**: your account's YouTube home feed, in YouTube's own order, as the first page.
- **Up next**: under every video, the recommendations YouTube would show you for it.
- **Autoplay**: when a video ends, the first Up next video starts (switch on the video page).
- **Watch history that teaches the account**: videos you play are marked watched on YouTube, so
  Home and Up next learn from what you watch here.
- **Window fullscreen**: besides normal fullscreen (`F`), a button and `T` make the video fill
  the window instead of the screen, like YouTube's theater mode.
- **One scrolling video page**: the player scrolls away with the description and Up next.

Playback itself stays anonymous: YouTube refuses signed-in stream requests that lack an extra
anti-bot token, so the account is only used to read recommendations and to mark videos watched.

> [!WARNING]
> Using an account from an unofficial client is against YouTube's terms and can get the account
> flagged. Use a separate (burner) account.

## Setup

1. Sign in to YouTube with your account in a Chromium profile.
2. Find the profile's folder name in `~/.config/chromium` (`Default`, `Profile 1`, ...).
3. Point Pipeline Home at it:

   ```sh
   gsettings set de.schmidhuberj.tubefeeder account-cookies-command "yt-account-cookies 'Profile 3'"
   ```

`account-cookies-command` is any command that prints the path to a Netscape `cookies.txt` of the
account. The bundled `yt-account-cookies` exports it from a Chromium profile through the GNOME keyring
(it needs `python-secretstorage` and `yt-dlp`). An empty command turns the account features off; Up
next then falls back to anonymous related videos.

## Install (Arch Linux)

```sh
git clone https://github.com/y-essine/pipeline-home
cd pipeline-home/packaging/arch
makepkg -si
```

It replaces `pipeline-gtk` and keeps the same app ID, so your subscriptions, watch later and
history carry over.

## Building

Same as Pipeline (Meson + Cargo, see [README.upstream.md](README.upstream.md)):

```sh
meson setup build -Dprofile=development
meson compile -C build
```

## What changed

The account code is kept apart so the patch stays easy to rebase onto new Pipeline releases:

- `src/backend/account.rs` (new): cookies, home feed (yt-dlp), Up next (RustyPipe, signed in),
  marking watched.
- `src/backend/client.rs`: `home_store`, `refresh_home()`, `up_next()`, `mark_watched_on_account()`.
- `src/gui/window.rs`, `data/resources/ui/window.blp`: the Home page.
- `src/gui/video_page.rs`, `data/resources/ui/video_page.blp`: Up next, autoplay, window
  fullscreen, single scrolling page.

The first commit is upstream Pipeline 4.1.1; everything after it is Pipeline Home.

## License

GPL-3.0-or-later, like Pipeline. All credit for Pipeline itself goes to its
[authors](https://gitlab.com/schmiddi-on-mobile/pipeline/-/graphs/main).
