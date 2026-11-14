//! Kinora: the signed-in YouTube account behind the Home page, the
//! "Up next" list and YouTube watch history.
//!
//! The account's cookies come from a user-configured command (the
//! `account-cookies-command` setting) that prints the path to a Netscape
//! cookies.txt. The account is only used for reading recommendations and for
//! marking videos as watched; playback stays anonymous, because YouTube
//! refuses signed-in stream requests that lack an extra anti-bot token.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::TimeDelta;
use pcore::{Channel, ChannelId, Video, VideoId};
use rustypipe::client::RustyPipe;
use tokio::process::Command;
use tokio::sync::{Mutex, OnceCell};
use url::Url;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

const PLATFORM: &str = "youtube";
const HOME_URL: &str = "https://www.youtube.com/feed/recommended";
/// How many videos the Home page asks for.
const HOME_SIZE: u32 = 60;
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

    let out = Command::new("sh").arg("-c").arg(command).output().await?;
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
    let out = Command::new("kinora-account")
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

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The account's YouTube home page, in YouTube's order. Upload dates and view
/// counts are not part of the listing; fill them in with the regular video
/// information lookup.
pub async fn home_feed(cookies: &Path) -> Result<Vec<Video>, Error> {
    let out = Command::new("yt-dlp")
        .arg("--cookies")
        .arg(cookies)
        .args(["--flat-playlist", "--no-warnings", "-J", "--playlist-end"])
        .arg(HOME_SIZE.to_string())
        .arg(HOME_URL)
        .output()
        .await?;
    if !out.status.success() {
        return Err(last_line(&out.stderr)
            .unwrap_or_else(|| "yt-dlp could not load the home feed".to_owned())
            .into());
    }

    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let entries = json["entries"].as_array().cloned().unwrap_or_default();
    Ok(entries.iter().filter_map(feed_entry_to_video).collect())
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

/// Mark a video as watched on the account, so its recommendations learn from it.
pub async fn mark_watched(cookies: &Path, video_id: &str) -> Result<(), Error> {
    let out = Command::new("yt-dlp")
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

async fn rusty_pipe(storage_dir: PathBuf) -> Result<RustyPipe, Error> {
    static RP: OnceCell<RustyPipe> = OnceCell::const_new();
    RP.get_or_try_init(|| async move {
        std::fs::create_dir_all(&storage_dir)?;
        Ok::<_, Error>(
            RustyPipe::builder()
                .storage_dir(storage_dir)
                .no_reporter()
                .build()?,
        )
    })
    .await
    .cloned()
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

fn feed_entry_to_video(e: &serde_json::Value) -> Option<Video> {
    let id = e["id"].as_str()?;
    // Only plain videos: Mixes and playlists ("YoutubeTab") are not playable as
    // a video and have no single channel.
    if e["ie_key"].as_str() != Some("Youtube") || id.len() != 11 {
        return None;
    }
    // Shorts are vertical and live in their own shelf; skip them.
    if e["url"].as_str().is_some_and(|u| u.contains("/shorts/")) {
        return None;
    }
    let title = e["title"].as_str()?;
    let channel_name = e["channel"]
        .as_str()
        .or(e["uploader"].as_str())
        .unwrap_or_default();
    let channel_id = e["channel_id"].as_str().unwrap_or_default();
    let thumbnail = e["thumbnails"]
        .as_array()
        .and_then(|t| t.iter().rev().find_map(|t| t["url"].as_str()))
        .and_then(|u| Url::parse(u).ok());

    Some(Video {
        id: VideoId {
            id: id.to_owned(),
            platform: PLATFORM.into(),
        },
        playable_url: watch_url(id),
        uploader: channel(channel_id, channel_name),
        title: title.to_owned(),
        // Not in the listing: the epoch means "not known yet" (shown as nothing)
        // until the video information is loaded.
        uploaded: chrono::DateTime::UNIX_EPOCH,
        duration: e["duration"].as_f64().map(|d| TimeDelta::seconds(d as i64)),
        thumbnail,
        description: None,
        views: e["view_count"].as_u64(),
        likes: None,
        dislikes: None,
        is_live: Some(e["live_status"].as_str() == Some("is_live")),
        requires_payment: Some(false),
    })
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

fn channel(id: &str, name: &str) -> Channel {
    Channel {
        id: ChannelId {
            id: id.to_owned(),
            platform: PLATFORM.into(),
        },
        name: name.to_owned(),
        avatar: None,
        banner: None,
        description: None,
        subscribers: None,
    }
}

fn watch_url(id: &str) -> Url {
    Url::parse_with_params("https://youtube.com/watch", &[("v", id)])
        .expect("YouTube URL parseable with video ID")
}

fn last_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .last()
        .map(str::to_owned)
}
