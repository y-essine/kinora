//! Kinora: the signed-in YouTube account behind the Home page, the
//! "Up next" list and YouTube watch history.
//!
//! The account's cookies come from a user-configured command (the
//! `account-cookies-command` setting) that prints the path to a Netscape
//! cookies.txt. The account is only used for reading recommendations and for
//! marking videos as watched; playback stays anonymous, because YouTube
//! refuses signed-in stream requests that lack an extra anti-bot token.
//!
//! The Home page itself is in [`crate::home`].

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::TimeDelta;
use pcore::{Channel, Video, VideoId};
use rustypipe::client::RustyPipe;
use tokio::sync::Mutex;
use url::Url;

pub use crate::Error;
use crate::playlist::{self, PlaylistCard, PlaylistKind};
use crate::youtube::{PLATFORM, channel, last_line, rusty_pipe, watch_url};

/// Reuse the cookies file this long before asking the command for a fresh one.
const COOKIES_TTL: Duration = Duration::from_secs(10 * 60);

/// Path to the account's cookies.txt, or `None` if no account is set up.
pub async fn cookies_file(command: &str) -> Result<Option<PathBuf>, Error> {
    let command = command.trim();
    if command.is_empty() {
        return Ok(None);
    }

    static CACHE: Mutex<Option<(String, Instant, PathBuf)>> = Mutex::const_new(None);
    let mut cache = CACHE.lock().await;
    if let Some((cmd, at, path)) = cache.as_ref() {
        if cmd == command && at.elapsed() < COOKIES_TTL && path.exists() {
            return Ok(Some(path.clone()));
        }
    }

    let out = crate::helper::shell(command).output().await?;
    if !out.status.success() {
        return Err(format!(
            "The account cookies command failed: {}",
            last_line(&out.stderr)
                .unwrap_or_else(|| format!("`{command}` exited with {}", out.status))
        )
        .into());
    }
    let path = last_line(&out.stdout)
        .map(PathBuf::from)
        .ok_or_else(|| format!("`{command}` printed no cookies file path"))?;
    if !path.exists() {
        return Err(format!("The account cookies file {} does not exist", path.display()).into());
    }

    *cache = Some((command.to_owned(), Instant::now(), path.clone()));
    Ok(Some(path))
}

/// A browser profile that is signed in to YouTube, as found by
/// `kinora-account --list`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BrowserProfile {
    /// Browser id understood by `kinora-account --browser`.
    pub browser: String,
    /// Human name of the browser, e.g. "Chromium".
    pub browser_name: String,
    /// Profile folder (Chromium family) or path (Firefox family).
    pub profile: String,
    /// The profile's own name, as shown in the browser.
    pub name: String,
}

impl BrowserProfile {
    /// The `account-cookies-command` that signs in through this profile.
    pub fn command(&self) -> String {
        format!(
            "kinora-account --browser {} --profile {}",
            shell_quote(&self.browser),
            shell_quote(&self.profile)
        )
    }

    /// How the account is shown once connected.
    pub fn label(&self) -> String {
        format!("{} · {}", self.name, self.browser_name)
    }
}

/// Browser profiles signed in to YouTube.
pub async fn browser_profiles() -> Result<Vec<BrowserProfile>, Error> {
    let out = crate::helper::command("kinora-account")
        .arg("--list")
        .output()
        .await
        .map_err(|e| {
            format!("Could not run kinora-account ({e}). Is Kinora installed completely?")
        })?;
    if !out.status.success() {
        return Err(last_line(&out.stderr)
            .unwrap_or_else(|| "kinora-account --list failed".to_owned())
            .into());
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

/// The `account-cookies-command` for an exported cookies.txt file.
pub fn file_command(path: &Path) -> String {
    format!(
        "kinora-account --file {}",
        shell_quote(&path.to_string_lossy())
    )
}

/// Run a cookies command once to check that it really yields a YouTube login.
pub async fn test_command(command: &str) -> Result<(), Error> {
    cookies_file(command)
        .await?
        .map(|_| ())
        .ok_or_else(|| "No command given".into())
}

#[cfg(not(windows))]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// On Windows the command runs in `cmd`, which knows no single quotes: double
/// quotes, as the helpers' (Python's) argument parsing reads them.
#[cfg(windows)]
fn shell_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}

/// YouTube's "Up next" for a video: personalised when an account is set up,
/// otherwise the anonymous related videos.
pub async fn up_next(
    storage_dir: PathBuf,
    cookies: Option<PathBuf>,
    video_id: String,
) -> Result<Vec<Video>, Error> {
    let rp = rusty_pipe(storage_dir).await?;

    let mut signed_in = false;
    if let Some(path) = cookies {
        match sign_in(&rp, &path).await {
            Ok(()) => signed_in = true,
            Err(e) => log::warn!("Up next: signing in failed, falling back to anonymous: {e}"),
        }
    }

    // One retry: a single dropped request shouldn't leave "Up next" empty.
    let mut attempt = 0;
    let details = loop {
        let query = if signed_in {
            rp.query().authenticated()
        } else {
            rp.query().unauthenticated()
        };
        match query.video_details(&video_id).await {
            Ok(details) => break details,
            Err(e) if attempt == 0 => {
                log::warn!("Up next: retrying after: {e}");
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(e) => return Err(e.into()),
        }
    };
    Ok(details
        .recommended
        .items
        .iter()
        .filter(|v| v.id != video_id && !v.is_short && !v.is_upcoming)
        .filter_map(video_item_to_video)
        .collect())
}

/// Every channel the account is subscribed to on YouTube, in YouTube's order.
pub async fn subscriptions(storage_dir: PathBuf, cookies: PathBuf) -> Result<Vec<Channel>, Error> {
    let rp = rusty_pipe(storage_dir).await?;
    sign_in(&rp, &cookies)
        .await
        .map_err(|e| format!("Signing in to YouTube failed: {e}"))?;

    let query = rp.query().authenticated();
    let mut channels = query.subscriptions().await?;
    channels.extend_all(&query).await?;

    Ok(channels
        .items
        .iter()
        .map(|c| {
            let mut channel = channel(&c.id, &c.name);
            channel.avatar = c.avatar.iter().rev().find_map(|t| Url::parse(&t.url).ok());
            channel.subscribers = c.subscriber_count;
            channel
        })
        .collect())
}

/// The most playlists [`playlists`] lists.
const PLAYLISTS_LIMIT: usize = 500;

/// The account's playlists: YouTube's "Playlists" page (`FEplaylist_aggregation`,
/// through RustyPipe's `saved_playlists`: the account's own playlists and the
/// ones it saved), then "Liked videos" (`LL`) and "Watch later" (`WL`) when
/// the page leaves them out (every account has them).
///
/// Not tried against a live account yet (Kinora's tests never use the
/// user's cookies): it relies on RustyPipe's parser of that page.
pub async fn playlists(storage_dir: PathBuf, cookies: PathBuf) -> Result<Vec<PlaylistCard>, Error> {
    let rp = rusty_pipe(storage_dir).await?;
    sign_in(&rp, &cookies)
        .await
        .map_err(|e| format!("Signing in to YouTube failed: {e}"))?;

    let query = rp.query().authenticated();
    let mut saved = query.saved_playlists().await?;
    saved.extend_limit(&query, PLAYLISTS_LIMIT).await?;
    let mut cards: Vec<PlaylistCard> = saved
        .items
        .iter()
        .filter_map(playlist::from_rustypipe)
        .collect();
    with_special_playlists(&mut cards);
    Ok(cards)
}

/// Add "Liked videos" and "Watch later" at the front when missing.
fn with_special_playlists(cards: &mut Vec<PlaylistCard>) {
    let special = [
        (playlist::LIKED_VIDEOS, "Liked videos"),
        (playlist::WATCH_LATER, "Watch later"),
    ];
    for (at, (id, title)) in special.into_iter().enumerate() {
        if cards.iter().any(|c| c.id == id) {
            continue;
        }
        cards.insert(
            at.min(cards.len()),
            PlaylistCard {
                id: id.to_owned(),
                title: title.to_owned(),
                kind: PlaylistKind::Playlist,
                thumbnail: None,
                owner: None,
                owner_id: None,
                video_count: None,
                first_video_id: None,
            },
        );
    }
}

/// Mark a video as watched on the account, so its recommendations learn from it.
pub async fn mark_watched(cookies: &Path, video_id: &str) -> Result<(), Error> {
    let out = crate::helper::command("yt-dlp")
        .arg("--cookies")
        .arg(cookies)
        .args(["--mark-watched", "--simulate", "--quiet", "--no-warnings"])
        .arg(format!("https://www.youtube.com/watch?v={video_id}"))
        .output()
        .await?;
    if out.status.success() {
        Ok(())
    } else {
        Err(last_line(&out.stderr)
            .unwrap_or_else(|| "yt-dlp could not mark the video watched".to_owned())
            .into())
    }
}

/// Hand the account's cookies to RustyPipe, again only when they changed.
async fn sign_in(rp: &RustyPipe, cookies: &Path) -> Result<(), Error> {
    static CURRENT: Mutex<Option<String>> = Mutex::const_new(None);
    let txt = tokio::fs::read_to_string(cookies).await?;
    let mut current = CURRENT.lock().await;
    if current.as_deref() != Some(txt.as_str()) {
        rp.user_auth_set_cookie_txt(&txt).await?;
        *current = Some(txt);
    }
    Ok(())
}

fn video_item_to_video(v: &rustypipe::model::VideoItem) -> Option<Video> {
    // YouTube's newer "lockup" layout leaves the channel out of the Up next
    // items; the caller fills it in afterwards from the video information.
    let uploader = match v.channel.as_ref() {
        Some(c) => {
            let mut uploader = channel(&c.id, &c.name);
            uploader.avatar = c.avatar.iter().rev().find_map(|t| Url::parse(&t.url).ok());
            uploader.subscribers = c.subscriber_count;
            uploader
        }
        None => channel("", ""),
    };

    Some(Video {
        id: VideoId {
            id: v.id.clone(),
            platform: PLATFORM.into(),
        },
        playable_url: watch_url(&v.id),
        uploader,
        title: v.name.clone(),
        uploaded: v
            .publish_date
            .and_then(|d| chrono::DateTime::from_timestamp(d.unix_timestamp(), 0))
            .unwrap_or(chrono::DateTime::UNIX_EPOCH),
        duration: v.duration.map(|s| TimeDelta::seconds(s as i64)),
        thumbnail: v
            .thumbnail
            .iter()
            .rev()
            .find_map(|t| Url::parse(&t.url).ok()),
        description: v.short_description.clone(),
        views: v.view_count,
        likes: None,
        dislikes: None,
        is_live: Some(v.is_live),
        requires_payment: Some(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str) -> PlaylistCard {
        PlaylistCard {
            id: id.to_owned(),
            title: id.to_owned(),
            kind: PlaylistKind::of(id),
            thumbnail: None,
            owner: None,
            owner_id: None,
            video_count: None,
            first_video_id: None,
        }
    }

    #[test]
    fn liked_videos_and_watch_later_are_always_listed() {
        let ids = |cards: &[PlaylistCard]| cards.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
        let mut cards = vec![card("PLa"), card("PLb")];
        with_special_playlists(&mut cards);
        assert_eq!(ids(&cards), ["LL", "WL", "PLa", "PLb"]);
        assert_eq!(cards[0].title, "Liked videos");

        // Listed already: left where YouTube put them.
        let mut cards = vec![card("PLa"), card("WL"), card("LL")];
        with_special_playlists(&mut cards);
        assert_eq!(ids(&cards), ["PLa", "WL", "LL"]);

        let mut none = Vec::new();
        with_special_playlists(&mut none);
        assert_eq!(ids(&none), ["LL", "WL"]);
    }
}
