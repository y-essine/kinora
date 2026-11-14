//! Kinora: the signed-in YouTube account behind the Home page, the
//! "Up next" list and YouTube watch history.
//!
//! The account's cookies come from a user-configured command (the
//! `account-cookies-command` setting) that prints the path to a Netscape
//! cookies.txt. The account is only used for reading recommendations and for
//! marking videos as watched; playback stays anonymous, because YouTube
//! refuses signed-in stream requests that lack an extra anti-bot token.
//!
//! The Home page is read straight from YouTube's InnerTube API (one request,
//! about a second); yt-dlp is the fallback when that fails.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::TimeDelta;
use pcore::{Channel, ChannelId, Video, VideoId};
use rustypipe::client::RustyPipe;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
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

const YOUTUBE_ORIGIN: &str = "https://www.youtube.com";
const BROWSE_URL: &str = "https://www.youtube.com/youtubei/v1/browse?prettyPrint=false";
/// The web client version RustyPipe falls back to; YouTube accepts it for browse requests.
const WEB_CLIENT_VERSION: &str = "2.20241216.05.00";
const WEB_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
/// How many continuation requests Home makes to get past pages without plain videos.
const HOME_EMPTY_PAGE_RETRIES: usize = 2;

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

/// The account's YouTube home page through yt-dlp, in YouTube's order: the slow
/// fallback for `home_page`. Upload dates and view counts are not part of the
/// listing; fill them in with the regular video information lookup.
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

/// A chip of the Home page's filter bar ("All", "Music", "Gaming", ...).
#[derive(Debug, Clone)]
pub struct HomeChip {
    pub title: String,
    /// The browse continuation that loads the chip's feed; `None` for "All".
    pub token: Option<String>,
}

/// One page of the Home feed.
#[derive(Debug, Clone, Default)]
pub struct HomePage {
    pub videos: Vec<Video>,
    /// The filter bar; only on the first page.
    pub chips: Vec<HomeChip>,
    /// Continuation for the next page, if YouTube has more.
    pub next: Option<String>,
}

/// The account's YouTube home page, in YouTube's order, straight from the
/// InnerTube API. Upload dates are not part of it; fill them in with the
/// regular video information lookup.
pub async fn home_page(cookies: &Path) -> Result<HomePage, Error> {
    let json = browse(cookies, json!({ "browseId": "FEwhat_to_watch" })).await?;
    let mut page = parse_home(&json);
    if page.videos.is_empty() && page.chips.is_empty() {
        return Err("YouTube's home page had no videos".into());
    }
    skip_empty_pages(cookies, &mut page).await;
    Ok(page)
}

/// A later page of the Home feed, or a chip's feed, from a continuation token.
pub async fn home_continuation(cookies: &Path, token: &str) -> Result<HomePage, Error> {
    let json = browse(cookies, json!({ "continuation": token })).await?;
    let mut page = parse_continuation(&json);
    skip_empty_pages(cookies, &mut page).await;
    Ok(page)
}

/// Pages made only of Shorts, Mixes and ads show nothing; look at the next one.
async fn skip_empty_pages(cookies: &Path, page: &mut HomePage) {
    for _ in 0..HOME_EMPTY_PAGE_RETRIES {
        let Some(token) = page.next.clone().filter(|_| page.videos.is_empty()) else {
            return;
        };
        match browse(cookies, json!({ "continuation": token })).await {
            Ok(json) => {
                let next = parse_continuation(&json);
                page.videos.extend(next.videos);
                page.next = next.next;
            }
            Err(e) => {
                log::debug!("Home: next page failed: {e}");
                return;
            }
        }
    }
}

/// A signed-in InnerTube `browse` request, as the web client (youtube.com) sends it.
async fn browse(cookies: &Path, request: Value) -> Result<Value, Error> {
    static HTTP: OnceCell<reqwest::Client> = OnceCell::const_new();
    let http = HTTP
        .get_or_try_init(|| async {
            reqwest::Client::builder()
                .user_agent(WEB_USER_AGENT)
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(20))
                .build()
        })
        .await?;

    let auth = Auth::from_cookies_txt(&tokio::fs::read_to_string(cookies).await?)?;
    let mut body = request;
    body["context"] = json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": WEB_CLIENT_VERSION,
            "hl": "en",
        }
    });

    let response = http
        .post(BROWSE_URL)
        .header("Content-Type", "application/json")
        .header("Cookie", auth.cookie)
        .header("Authorization", sapisidhash(&auth.sapisid))
        .header("Origin", YOUTUBE_ORIGIN)
        .header("X-Origin", YOUTUBE_ORIGIN)
        .header("X-Goog-AuthUser", "0")
        .body(body.to_string())
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("YouTube answered the home request with {status}").into());
    }
    Ok(serde_json::from_slice(&response.bytes().await?)?)
}

/// What a signed-in request needs from the cookies.txt.
struct Auth {
    /// The `Cookie` header: the youtube.com cookies.
    cookie: String,
    sapisid: String,
}

impl Auth {
    fn from_cookies_txt(txt: &str) -> Result<Self, Error> {
        let mut pairs = Vec::new();
        let mut sapisid = None;
        for line in txt.lines() {
            // Chromium-style exports mark HttpOnly cookies with this prefix.
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            if line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 7
                || !matches!(
                    fields[0].trim_start_matches('.'),
                    "youtube.com" | "www.youtube.com"
                )
            {
                continue;
            }
            let (name, value) = (fields[5], fields[6].trim_end());
            if name == "SAPISID" {
                sapisid = Some(value.to_owned());
            }
            pairs.push(format!("{name}={value}"));
        }
        let sapisid = sapisid
            .ok_or("The account cookies hold no YouTube login (SAPISID). Connect it again.")?;
        Ok(Self {
            cookie: pairs.join("; "),
            sapisid,
        })
    }
}

/// The `Authorization` header YouTube's web client sends when signed in.
fn sapisidhash(sapisid: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut hash = Sha1::new();
    hash.update(format!("{now} {sapisid} {YOUTUBE_ORIGIN}"));
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("SAPISIDHASH {now}_{hex}")
}

/// The first page: a rich grid with a filter chip bar above it.
fn parse_home(json: &Value) -> HomePage {
    let grid = json["contents"]["twoColumnBrowseResultsRenderer"]["tabs"]
        .as_array()
        .and_then(|tabs| {
            tabs.iter()
                .find_map(|t| t["tabRenderer"]["content"].get("richGridRenderer"))
        });
    let Some(grid) = grid else {
        return HomePage::default();
    };

    let mut page = parse_items(grid["contents"].as_array().map_or(&[], Vec::as_slice));
    page.chips = grid["header"]["feedFilterChipBarRenderer"]["contents"]
        .as_array()
        .map(|chips| chips.iter().filter_map(parse_chip).collect())
        .unwrap_or_default();
    page
}

fn parse_chip(chip: &Value) -> Option<HomeChip> {
    let chip = chip.get("chipCloudChipRenderer")?;
    let title = text_of(&chip["text"])?;
    let token = chip["navigationEndpoint"]["continuationCommand"]["token"]
        .as_str()
        .map(str::to_owned);
    // Anything but the selected "All" chip needs a way to load it.
    if token.is_none() && chip["isSelected"].as_bool() != Some(true) {
        return None;
    }
    Some(HomeChip { title, token })
}

/// A continuation response: the next page, or a chip's feed.
fn parse_continuation(json: &Value) -> HomePage {
    let mut page = HomePage::default();
    for action in json["onResponseReceivedActions"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let items = [
            "reloadContinuationItemsCommand",
            "appendContinuationItemsAction",
        ]
        .iter()
        .find_map(|k| action[k]["continuationItems"].as_array());
        if let Some(items) = items {
            let part = parse_items(items);
            page.videos.extend(part.videos);
            page.next = part.next.or(page.next);
        }
    }
    page
}

/// Videos and the continuation out of a list of grid items. YouTube currently
/// sends videos as `lockupViewModel`, older responses as `videoRenderer`.
fn parse_items(items: &[Value]) -> HomePage {
    let mut page = HomePage::default();
    for item in items {
        if let Some(token) =
            item["continuationItemRenderer"]["continuationEndpoint"]["continuationCommand"]["token"]
                .as_str()
        {
            page.next = Some(token.to_owned());
            continue;
        }
        // Shelves (Shorts, ...) are `richSectionRenderer`s and ads `adSlotRenderer`s: skipped.
        let content = &item["richItemRenderer"]["content"];
        let video = if let Some(lockup) = content.get("lockupViewModel") {
            lockup_to_video(lockup)
        } else if let Some(renderer) = content.get("videoRenderer") {
            video_renderer_to_video(renderer)
        } else {
            None
        };
        page.videos.extend(video);
    }
    page
}

fn lockup_to_video(l: &Value) -> Option<Video> {
    // Mixes and playlists are lockups too.
    if l["contentType"].as_str() != Some("LOCKUP_CONTENT_TYPE_VIDEO") {
        return None;
    }
    let id = l["contentId"].as_str()?;
    let meta = &l["metadata"]["lockupMetadataViewModel"];
    let title = meta["title"]["content"].as_str()?;

    // The channel is the part that links to a channel.
    let parts: Vec<&Value> = meta["metadata"]["contentMetadataViewModel"]["metadataRows"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| row["metadataParts"].as_array().into_iter().flatten())
        .collect();
    let channel_part = parts.iter().find_map(|part| {
        part["text"]["commandRuns"]
            .as_array()?
            .iter()
            .find_map(|run| run["onTap"]["innertubeCommand"]["browseEndpoint"]["browseId"].as_str())
            .map(|browse_id| (browse_id, part["text"]["content"].as_str()))
    });
    let channel_id = channel_part.map_or("", |(id, _)| id);
    let channel_name = channel_part
        .and_then(|(_, name)| name)
        .or_else(|| parts.first().and_then(|p| p["text"]["content"].as_str()))
        .unwrap_or_default();

    let mut views = None;
    let mut is_live = false;
    for text in parts.iter().filter_map(|p| p["text"]["content"].as_str()) {
        let lower = text.to_lowercase();
        // Not out yet: scheduled streams and premieres.
        if lower.starts_with("scheduled") || lower.contains("waiting") || lower.contains("premiere")
        {
            return None;
        }
        if lower.ends_with("watching") {
            is_live = true;
            views = views.or_else(|| parse_count(text));
        } else if lower.ends_with("views") || lower.ends_with("view") {
            views = parse_count(text);
        }
    }

    let thumbnail_view = &l["contentImage"]["thumbnailViewModel"];
    let mut duration = None;
    let mut badges = Vec::new();
    collect_badges(&thumbnail_view["overlays"], &mut badges);
    for (text, style) in badges {
        if style.contains("UPCOMING") || text.eq_ignore_ascii_case("upcoming") {
            return None;
        }
        if style.contains("LIVE") || text.eq_ignore_ascii_case("live") {
            is_live = true;
        } else if let Some(d) = parse_duration(&text) {
            duration = Some(d);
        }
    }
    // Neither a length nor live: a premiere or something else that cannot be played yet.
    if duration.is_none() && !is_live {
        return None;
    }

    let mut uploader = channel(channel_id, channel_name);
    uploader.avatar = best_url(
        &meta["image"]["decoratedAvatarViewModel"]["avatar"]["avatarViewModel"]["image"]["sources"],
    );
    Some(feed_video(
        id,
        title,
        uploader,
        duration,
        best_url(&thumbnail_view["image"]["sources"]),
        views,
        is_live,
    ))
}

/// The older `videoRenderer` shape.
fn video_renderer_to_video(v: &Value) -> Option<Video> {
    let id = v["videoId"].as_str()?;
    let title = text_of(&v["title"])?;
    if v.get("upcomingEventData").is_some() {
        return None;
    }

    let mut is_live = false;
    for overlay in v["thumbnailOverlays"].as_array().into_iter().flatten() {
        match overlay["thumbnailOverlayTimeStatusRenderer"]["style"].as_str() {
            Some("UPCOMING" | "SHORTS") => return None,
            Some("LIVE") => is_live = true,
            _ => {}
        }
    }
    let duration = text_of(&v["lengthText"]).and_then(|t| parse_duration(&t));
    if duration.is_none() && !is_live {
        return None;
    }

    let owner = v["ownerText"]["runs"]
        .get(0)
        .or(v["longBylineText"]["runs"].get(0));
    let channel_name = owner.and_then(|r| r["text"].as_str()).unwrap_or_default();
    let channel_id = owner
        .and_then(|r| r["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str())
        .unwrap_or_default();
    Some(feed_video(
        id,
        &title,
        channel(channel_id, channel_name),
        duration,
        best_url(&v["thumbnail"]["thumbnails"]),
        text_of(&v["viewCountText"]).and_then(|t| parse_count(&t)),
        is_live,
    ))
}

fn feed_video(
    id: &str,
    title: &str,
    uploader: Channel,
    duration: Option<TimeDelta>,
    thumbnail: Option<Url>,
    views: Option<u64>,
    is_live: bool,
) -> Video {
    Video {
        id: VideoId {
            id: id.to_owned(),
            platform: PLATFORM.into(),
        },
        playable_url: watch_url(id),
        uploader,
        title: title.to_owned(),
        // Only "3 days ago" is in the listing: the epoch means "not known yet" (shown
        // as nothing) until the video information is loaded.
        uploaded: chrono::DateTime::UNIX_EPOCH,
        duration,
        thumbnail,
        description: None,
        views,
        likes: None,
        dislikes: None,
        is_live: Some(is_live),
        requires_payment: Some(false),
    }
}

/// `(text, style)` of every thumbnail badge below `value`.
fn collect_badges(value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            if let Some(badge) = map.get("thumbnailBadgeViewModel") {
                out.push((
                    badge["text"].as_str().unwrap_or_default().to_owned(),
                    badge["badgeStyle"].as_str().unwrap_or_default().to_owned(),
                ));
            }
            map.values().for_each(|v| collect_badges(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| collect_badges(v, out)),
        _ => {}
    }
}

/// The largest image of a list of `{ "url": ... }` sources (they are sorted by size).
fn best_url(sources: &Value) -> Option<Url> {
    sources
        .as_array()?
        .iter()
        .rev()
        .find_map(|s| s["url"].as_str())
        .and_then(|u| Url::parse(u).ok())
}

/// Text of `{ "simpleText": ... }` or `{ "runs": [...] }`.
fn text_of(v: &Value) -> Option<String> {
    let text = match v["simpleText"].as_str() {
        Some(t) => t.to_owned(),
        None => v["runs"]
            .as_array()?
            .iter()
            .filter_map(|r| r["text"].as_str())
            .collect(),
    };
    Some(text).filter(|t| !t.is_empty())
}

/// "12:48" or "1:09:10".
fn parse_duration(text: &str) -> Option<TimeDelta> {
    let mut seconds = 0i64;
    for part in text.trim().split(':') {
        seconds = seconds * 60 + part.parse::<i64>().ok()?;
    }
    Some(TimeDelta::seconds(seconds))
}

/// "980K views", "1,234 views", "5.2K watching", "No views".
fn parse_count(text: &str) -> Option<u64> {
    let word = text.split_whitespace().next()?;
    if word.eq_ignore_ascii_case("no") {
        return Some(0);
    }
    let (number, scale) = match word.chars().last()? {
        'K' | 'k' => (&word[..word.len() - 1], 1e3),
        'M' => (&word[..word.len() - 1], 1e6),
        'B' => (&word[..word.len() - 1], 1e9),
        _ => (word, 1.0),
    };
    let number: f64 = number.replace(',', "").parse().ok()?;
    Some((number * scale).round() as u64)
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

pub(crate) async fn rusty_pipe(storage_dir: PathBuf) -> Result<RustyPipe, Error> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lockup(kind: &str, id: &str, rows: Value, badge: Option<(&str, &str)>) -> Value {
        let overlays: Vec<Value> = badge
            .map(|(text, style)| {
                json!({ "thumbnailBottomOverlayViewModel": { "badges": [
                    { "thumbnailBadgeViewModel": { "text": text, "badgeStyle": style } }
                ] } })
            })
            .into_iter()
            .collect();
        json!({ "richItemRenderer": { "content": { "lockupViewModel": {
            "contentType": kind,
            "contentId": id,
            "contentImage": { "thumbnailViewModel": {
                "image": { "sources": [
                    { "url": "https://i.ytimg.com/small.jpg" },
                    { "url": "https://i.ytimg.com/big.jpg" },
                ] },
                "overlays": overlays,
            } },
            "metadata": { "lockupMetadataViewModel": {
                "title": { "content": "A title" },
                "metadata": { "contentMetadataViewModel": { "metadataRows": rows } },
            } },
        } } } })
    }

    fn rows(views: &str) -> Value {
        json!([
            { "metadataParts": [ { "text": { "content": "A channel", "commandRuns": [
                { "onTap": { "innertubeCommand": { "browseEndpoint": { "browseId": "UC123" } } } }
            ] } } ] },
            { "metadataParts": [
                { "text": { "content": views } },
                { "text": { "content": "1 day ago" } },
            ] },
        ])
    }

    const DEFAULT: &str = "THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT";

    #[test]
    fn plain_lockup_is_a_video() {
        let items = [lockup(
            "LOCKUP_CONTENT_TYPE_VIDEO",
            "abcdefghijk",
            rows("3.6M views"),
            Some(("1:02:03", DEFAULT)),
        )];
        let v = &parse_items(&items).videos[0];
        assert_eq!(v.id.id, "abcdefghijk");
        assert_eq!(v.title, "A title");
        assert_eq!(v.uploader.name, "A channel");
        assert_eq!(v.uploader.id.id, "UC123");
        assert_eq!(v.duration, Some(TimeDelta::seconds(3723)));
        assert_eq!(v.views, Some(3_600_000));
        assert_eq!(v.is_live, Some(false));
        assert_eq!(v.thumbnail.as_ref().unwrap().path(), "/big.jpg");
        assert_eq!(v.uploaded, chrono::DateTime::UNIX_EPOCH);
    }

    #[test]
    fn mixes_upcoming_and_ads_are_skipped() {
        let items = [
            lockup("LOCKUP_CONTENT_TYPE_PLAYLIST", "RDabc", rows("x"), None),
            lockup(
                "LOCKUP_CONTENT_TYPE_VIDEO",
                "abcdefghijk",
                rows("Scheduled for 5/5/27"),
                Some(("UPCOMING", "THUMBNAIL_OVERLAY_BADGE_STYLE_UPCOMING")),
            ),
            lockup(
                "LOCKUP_CONTENT_TYPE_VIDEO",
                "abcdefghijk",
                rows("1 view"),
                None,
            ),
            json!({ "richItemRenderer": { "content": { "adSlotRenderer": {} } } }),
            json!({ "richSectionRenderer": { "content": { "richShelfRenderer": {} } } }),
        ];
        assert!(parse_items(&items).videos.is_empty());
    }

    #[test]
    fn live_streams_are_kept() {
        let items = [lockup(
            "LOCKUP_CONTENT_TYPE_VIDEO",
            "abcdefghijk",
            rows("5.2K watching"),
            Some(("LIVE", "THUMBNAIL_OVERLAY_BADGE_STYLE_LIVE")),
        )];
        let v = &parse_items(&items).videos[0];
        assert_eq!(v.is_live, Some(true));
        assert_eq!(v.views, Some(5200));
        assert_eq!(v.duration, None);
    }

    #[test]
    fn old_video_renderer_is_a_video() {
        let items = [
            json!({ "richItemRenderer": { "content": { "videoRenderer": {
            "videoId": "abcdefghijk",
            "title": { "runs": [ { "text": "Old " }, { "text": "title" } ] },
            "ownerText": { "runs": [ { "text": "Chan", "navigationEndpoint": {
                "browseEndpoint": { "browseId": "UC9" } } } ] },
            "lengthText": { "simpleText": "12:48" },
            "viewCountText": { "simpleText": "1,234 views" },
            "thumbnail": { "thumbnails": [ { "url": "https://i.ytimg.com/a.jpg" } ] },
        } } } }),
        ];
        let v = &parse_items(&items).videos[0];
        assert_eq!(v.title, "Old title");
        assert_eq!(v.uploader.id.id, "UC9");
        assert_eq!(v.duration, Some(TimeDelta::seconds(768)));
        assert_eq!(v.views, Some(1234));
    }

    #[test]
    fn chips_and_continuation() {
        let chip = |title: &str, token: Option<&str>, selected: bool| {
            json!({ "chipCloudChipRenderer": {
                "text": { "runs": [ { "text": title } ] },
                "isSelected": selected,
                "navigationEndpoint": token.map(|t| json!({ "continuationCommand": { "token": t } })),
            } })
        };
        let home = json!({ "contents": { "twoColumnBrowseResultsRenderer": { "tabs": [
            { "tabRenderer": { "content": { "richGridRenderer": {
                "header": { "feedFilterChipBarRenderer": { "contents": [
                    chip("All", None, true),
                    chip("Music", Some("tok-music"), false),
                    chip("Broken", None, false),
                ] } },
                "contents": [ { "continuationItemRenderer": { "continuationEndpoint": {
                    "continuationCommand": { "token": "tok-next" } } } } ],
            } } } }
        ] } } });
        let page = parse_home(&home);
        assert_eq!(page.chips.len(), 2);
        assert_eq!(page.chips[0].title, "All");
        assert_eq!(page.chips[0].token, None);
        assert_eq!(page.chips[1].token.as_deref(), Some("tok-music"));
        assert_eq!(page.next.as_deref(), Some("tok-next"));

        let more = json!({ "onResponseReceivedActions": [ { "appendContinuationItemsAction": {
            "continuationItems": [ lockup(
                "LOCKUP_CONTENT_TYPE_VIDEO", "abcdefghijk", rows("1 view"), Some(("0:30", DEFAULT))
            ) ] } } ] });
        assert_eq!(parse_continuation(&more).videos.len(), 1);
    }

    #[test]
    fn counts_and_durations() {
        assert_eq!(parse_count("980K views"), Some(980_000));
        assert_eq!(parse_count("1.8M views"), Some(1_800_000));
        assert_eq!(parse_count("1,234 views"), Some(1234));
        assert_eq!(parse_count("No views"), Some(0));
        assert_eq!(parse_count("views"), None);
        assert_eq!(parse_duration("0:30"), Some(TimeDelta::seconds(30)));
        assert_eq!(parse_duration("LIVE"), None);
    }

    #[test]
    fn cookies_txt_gives_a_header_and_sapisid() {
        let txt = "# Netscape HTTP Cookie File\n\
            .youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tsecret\n\
            #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc\n\
            .google.com\tTRUE\t/\tTRUE\t0\tNID\tnope\n";
        let auth = Auth::from_cookies_txt(txt).unwrap();
        assert_eq!(auth.sapisid, "secret");
        assert_eq!(auth.cookie, "SAPISID=secret; SID=abc");
        assert!(Auth::from_cookies_txt(".google.com\tTRUE\t/\tTRUE\t0\tNID\tx").is_err());
        assert!(sapisidhash("secret").starts_with("SAPISIDHASH "));
    }
}
