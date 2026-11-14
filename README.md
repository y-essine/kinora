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

## Connecting an account (optional)

Pipeline Home starts **fully anonymous**. To add your Home feed and personal Up next:

1. Sign in to YouTube in your browser: Chromium, Chrome, Brave, Vivaldi, Edge, Firefox or LibreWolf.
2. In Pipeline Home, press **Connect Account** on the Home page (or *Preferences → YouTube Account*).
3. Pick the browser profile from the list. That's it.

You can also use an exported `cookies.txt` file. *Disconnect* in the preferences goes back to fully
anonymous.

<details>
<summary>How it works, and the command line</summary>

The bundled `yt-account-cookies` finds browser profiles that are signed in to YouTube (it only checks
for the login cookie's presence, nothing is decrypted to list them) and exports the chosen one's
YouTube cookies to a private file in `~/.cache/pipeline-home`. Chromium-family browsers keep their
cookie key in the GNOME keyring; Firefox-family browsers need no keyring.

```sh
yt-account-cookies --list
gsettings set de.schmidhuberj.tubefeeder account-cookies-command \
  "yt-account-cookies --browser chromium --profile 'Profile 3'"
```

`account-cookies-command` can be any command that prints the path of a Netscape `cookies.txt`.
</details>

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
