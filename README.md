<div align="center">
 <h1>🎞️ Kinora</h1>
 <strong>Watch YouTube and PeerTube your way: fully anonymous, lightweight, or with your account</strong>
</div>
<br>

Kinora is a GTK app for Linux for watching YouTube and PeerTube without the website. It starts out
fully anonymous, and can optionally use a YouTube account for the things that need one. It is based
on [Pipeline](https://gitlab.com/schmiddi-on-mobile/pipeline) by Julian Schmidhuber and contributors
(and can be installed next to it), and adds:

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

## Connecting an account (optional)

Kinora starts **fully anonymous**. To add your Home feed and personal Up next:

1. Sign in to YouTube in your browser: Chromium, Chrome, Brave, Vivaldi, Edge, Firefox or LibreWolf.
2. In Kinora, press **Connect Account** on the Home page (or *Preferences → YouTube Account*).
3. Pick the browser profile from the list. That's it.

You can also use an exported `cookies.txt` file. *Disconnect* in the preferences goes back to fully
anonymous.

<details>
<summary>How it works, and the command line</summary>

The bundled `kinora-account` finds browser profiles that are signed in to YouTube (it only checks
for the login cookie's presence, nothing is decrypted to list them) and exports the chosen one's
YouTube cookies to a private file in `~/.cache/kinora`. Chromium-family browsers keep their
cookie key in the GNOME keyring; Firefox-family browsers need no keyring.

```sh
kinora-account --list
gsettings set io.github.y_essine.Kinora account-cookies-command \
  "kinora-account --browser chromium --profile 'Profile 3'"
```

`account-cookies-command` can be any command that prints the path of a Netscape `cookies.txt`.
</details>

## Install (Arch Linux)

```sh
git clone https://github.com/y-essine/kinora
cd kinora/packaging/arch
makepkg -si
```

Kinora has its own app ID, so it installs next to Pipeline. On first start it copies Pipeline's
library (subscriptions, watch later, history) and settings, leaving Pipeline's own files as they are.

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

The first commit is upstream Pipeline 4.1.1; everything after it is Kinora.

## The name

A *kinora* was an 1890s hand-cranked flip-book movie viewer: a small, personal way to watch moving pictures.

## License

GPL-3.0-or-later, like Pipeline. All credit for Pipeline itself goes to its
[authors](https://gitlab.com/schmiddi-on-mobile/pipeline/-/graphs/main).
