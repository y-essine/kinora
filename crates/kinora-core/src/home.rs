//! Kinora: the account's YouTube Home page, with its filter chips.
//!
//! Read straight from YouTube's InnerTube API (one request, about a second);
//! yt-dlp is the fallback when that fails.
//!
//! A page holds plain videos ([`HomePage::videos`]) and, apart from them,
//! the playlists and mixes of the grid ([`HomePage::playlists`]). The
//! "Mixes" and "Playlists" chips hold only the latter. Callers that show
//! playlists ask with `playlists: true` (see [`load`]), so a page made only
//! of playlists counts as a page rather than as an empty one to skip.

use std::path::Path;
use std::time::Duration;

use chrono::TimeDelta;
use pcore::{Channel, Video, VideoId};
use serde_json::{Value, json};
use tokio::sync::OnceCell;
use url::Url;

use crate::Error;
use crate::playlist::{self, PlaylistCard, PlaylistKind};
use crate::youtube::{
    Auth, PLATFORM, WEB_USER_AGENT, best_url, channel, last_line, parse_count, parse_duration,
    text_of, watch_url,
};

const HOME_URL: &str = "https://www.youtube.com/feed/recommended";
/// How many videos the Home page asks for.
const HOME_SIZE: u32 = 60;

const YOUTUBE_ORIGIN: &str = "https://www.youtube.com";
const BROWSE_URL: &str = "https://www.youtube.com/youtubei/v1/browse?prettyPrint=false";
/// The web client version RustyPipe falls back to; YouTube accepts it for browse requests.
pub(crate) const WEB_CLIENT_VERSION: &str = "2.20241216.05.00";
/// How many continuation requests Home makes to get past pages without plain videos.
const HOME_EMPTY_PAGE_RETRIES: usize = 2;

/// The first page of Home: the plain home page, or a chip's feed when
/// `chip_token` is given. The plain page falls back to yt-dlp (slower) when the
/// direct request fails or comes back without videos.
///
/// `playlists`: the caller shows [`HomePage::playlists`] too, so a page with
/// playlists and no videos is not empty (it is neither skipped nor a reason
/// for the yt-dlp fallback). Without it, pages without videos are skipped as
/// before; the playlists of the pages seen are kept either way.
pub async fn load(
    cookies: &Path,
    chip_token: Option<&str>,
    playlists: bool,
) -> Result<HomePage, Error> {
    if let Some(token) = chip_token {
        return continuation(cookies, token, playlists).await;
    }
    match first_page(cookies, playlists).await {
        Ok(page) if !page.is_empty(playlists) => Ok(page),
        // The direct request failed or came back empty: ask yt-dlp (slower).
        direct => {
            match direct {
                Ok(_) => log::warn!("Home: YouTube sent no videos, trying yt-dlp"),
                Err(e) => log::warn!("Home: direct request failed, trying yt-dlp: {e}"),
            }
            ytdlp_page(cookies).await
        }
    }
}

/// The account's YouTube home page through yt-dlp, in YouTube's order: the slow
/// fallback for `first_page`. Upload dates and view counts are not part of the
/// listing; fill them in with the regular video information lookup.
pub async fn ytdlp_feed(cookies: &Path) -> Result<Vec<Video>, Error> {
    ytdlp_page(cookies).await.map(|page| page.videos)
}

/// [`ytdlp_feed`] with the listing's playlists and mixes.
async fn ytdlp_page(cookies: &Path) -> Result<HomePage, Error> {
    let out = crate::helper::command("yt-dlp")
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
    let entries = json["entries"].as_array().map_or(&[][..], Vec::as_slice);
    Ok(HomePage {
        videos: entries.iter().filter_map(feed_entry_to_video).collect(),
        playlists: entries.iter().filter_map(feed_entry_to_playlist).collect(),
        ..Default::default()
    })
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
    /// Plain videos (no Shorts, ads, upcoming streams, playlists or mixes).
    pub videos: Vec<Video>,
    /// The page's playlists, mixes and albums, in YouTube's order, apart
    /// from the videos. The "Mixes" and "Playlists" chips' feeds hold only
    /// these. Play one with [`PlaylistCard::dj_seed`].
    pub playlists: Vec<PlaylistCard>,
    /// The filter bar; only on the first page.
    pub chips: Vec<HomeChip>,
    /// Continuation for the next page, if YouTube has more.
    pub next: Option<String>,
}

impl HomePage {
    /// Nothing to show: no videos, and, when the caller shows them
    /// (`playlists`), no playlists either.
    pub fn is_empty(&self, playlists: bool) -> bool {
        self.videos.is_empty() && (!playlists || self.playlists.is_empty())
    }
}

/// The account's YouTube home page, in YouTube's order, straight from the
/// InnerTube API. Upload dates are not part of it; fill them in with the
/// regular video information lookup. `playlists`: see [`load`].
pub async fn first_page(cookies: &Path, playlists: bool) -> Result<HomePage, Error> {
    let json = browse(cookies, json!({ "browseId": "FEwhat_to_watch" })).await?;
    let mut page = parse_home(&json);
    if page.is_empty(true) && page.chips.is_empty() {
        return Err("YouTube's home page had no videos".into());
    }
    skip_empty_pages(cookies, &mut page, playlists).await;
    Ok(page)
}

/// A later page of the Home feed, or a chip's feed, from a continuation
/// token. `playlists`: see [`load`].
pub async fn continuation(cookies: &Path, token: &str, playlists: bool) -> Result<HomePage, Error> {
    let json = browse(cookies, json!({ "continuation": token })).await?;
    let mut page = parse_continuation(&json);
    skip_empty_pages(cookies, &mut page, playlists).await;
    Ok(page)
}

/// Pages made only of Shorts, Mixes and ads show nothing in a list of
/// videos; look at the next one. With `playlists`, Mixes are something to
/// show, so only pages with neither are skipped.
async fn skip_empty_pages(cookies: &Path, page: &mut HomePage, playlists: bool) {
    for _ in 0..HOME_EMPTY_PAGE_RETRIES {
        let Some(token) = page.next.clone().filter(|_| page.is_empty(playlists)) else {
            return;
        };
        match browse(cookies, json!({ "continuation": token })).await {
            Ok(json) => {
                let next = parse_continuation(&json);
                page.videos.extend(next.videos);
                page.playlists.extend(next.playlists);
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

    let auth = Auth::from_file(cookies, "www.youtube.com").await?;
    let mut body = request;
    body["context"] = json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": WEB_CLIENT_VERSION,
            "hl": "en",
        }
    });

    let response = auth
        .sign(http.post(BROWSE_URL), YOUTUBE_ORIGIN)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("YouTube answered the home request with {status}").into());
    }
    Ok(serde_json::from_slice(&response.bytes().await?)?)
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
            page.playlists.extend(part.playlists);
            page.next = part.next.or(page.next);
        }
    }
    page
}

/// Videos, playlists and the continuation out of a list of grid items.
/// YouTube currently sends videos and playlists as `lockupViewModel`, older
/// responses videos as `videoRenderer`.
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
            if let Some(card) = playlist::from_lockup(lockup) {
                page.playlists.push(card);
                continue;
            }
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
    // Mixes and playlists are lockups too (`playlist::from_lockup`).
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

/// A playlist or mix of the yt-dlp listing (a "YoutubeTab" entry with a
/// playlist id).
fn feed_entry_to_playlist(e: &serde_json::Value) -> Option<PlaylistCard> {
    if e["ie_key"].as_str() != Some("YoutubeTab") {
        return None;
    }
    let id = e["url"]
        .as_str()
        .filter(|u| u.contains("list="))
        .or(e["id"].as_str())
        .and_then(playlist::normalize_id)?;
    // Channels are "YoutubeTab" entries too.
    if id.starts_with("UC") && id.len() == 24 {
        return None;
    }
    Some(PlaylistCard {
        kind: PlaylistKind::of(&id),
        title: e["title"].as_str()?.to_owned(),
        thumbnail: e["thumbnails"]
            .as_array()
            .and_then(|t| t.iter().rev().find_map(|t| t["url"].as_str()))
            .and_then(|u| Url::parse(u).ok()),
        owner: e["channel"]
            .as_str()
            .or(e["uploader"].as_str())
            .map(str::to_owned),
        owner_id: e["channel_id"].as_str().map(str::to_owned),
        video_count: e["playlist_count"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok()),
        first_video_id: None,
        id,
    })
}

fn feed_entry_to_video(e: &serde_json::Value) -> Option<Video> {
    let id = e["id"].as_str()?;
    // Only plain videos: Mixes and playlists ("YoutubeTab",
    // `feed_entry_to_playlist`) are not playable as a video and have no
    // single channel.
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
        let page = parse_items(&items);
        assert!(page.videos.is_empty());
        // The mix is not a video, but it is not lost either.
        assert_eq!(page.playlists.len(), 1);
        assert_eq!(page.playlists[0].id, "RDabc");
        assert_eq!(page.playlists[0].kind, PlaylistKind::Mix);
        assert!(page.is_empty(false));
        assert!(!page.is_empty(true));
    }

    /// Like a "Mixes" chip's feed: a continuation of playlist lockups and one
    /// video (real lockups from an anonymous search, in Home's grid items).
    #[test]
    fn a_page_of_mixes_has_playlists_and_no_videos() {
        let fixture: Value =
            serde_json::from_str(include_str!("fixtures/playlist_lockups.json")).unwrap();
        let mut items: Vec<Value> = fixture["lockups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| json!({ "richItemRenderer": { "content": { "lockupViewModel": l } } }))
            .collect();
        items.push(
            json!({ "continuationItemRenderer": { "continuationEndpoint": {
            "continuationCommand": { "token": "tok-next" } } } }),
        );
        let more = json!({ "onResponseReceivedActions": [
            { "reloadContinuationItemsCommand": { "continuationItems": items } }
        ] });
        let page = parse_continuation(&more);
        // The one video among them stays a video.
        assert_eq!(page.videos.len(), 1);
        assert_eq!(page.videos[0].id.id, "a5uQMwRMHcs");
        assert_eq!(page.playlists.len(), 4);
        assert_eq!(
            page.playlists.iter().map(|p| p.kind).collect::<Vec<_>>(),
            [
                PlaylistKind::Playlist,
                PlaylistKind::Mix,
                PlaylistKind::Mix,
                PlaylistKind::Album
            ]
        );
        assert_eq!(page.next.as_deref(), Some("tok-next"));
        assert!(!page.is_empty(true));
    }

    #[test]
    fn ytdlp_playlists_are_cards() {
        let entries = json!([
            { "ie_key": "YoutubeTab", "id": "RDdQw4w9WgXcQ", "title": "Mix - Rick Astley",
              "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=RDdQw4w9WgXcQ" },
            { "ie_key": "YoutubeTab", "id": "PLabc", "title": "Mine", "playlist_count": 12,
              "channel": "Me", "url": "https://www.youtube.com/playlist?list=PLabc" },
            { "ie_key": "YoutubeTab", "id": "UCuAXFkgsw1L7xaCfnd5JJOw", "title": "A channel",
              "url": "https://www.youtube.com/channel/UCuAXFkgsw1L7xaCfnd5JJOw" },
            { "ie_key": "Youtube", "id": "dQw4w9WgXcQ", "title": "A video" },
        ]);
        let cards: Vec<PlaylistCard> = entries
            .as_array()
            .unwrap()
            .iter()
            .filter_map(feed_entry_to_playlist)
            .collect();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].kind, PlaylistKind::Mix);
        assert_eq!(cards[1].id, "PLabc");
        assert_eq!(cards[1].video_count, Some(12));
        assert_eq!(cards[1].owner.as_deref(), Some("Me"));
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
    fn chips_are_parsed_with_their_token() {
        // Skipping the playlist-only chips (Mixes) is the home-chips
        // extension's job, not the parser's.
        let chip = |t: &str| {
            json!({ "chipCloudChipRenderer": {
                "text": { "runs": [{ "text": t }] },
                "navigationEndpoint": { "continuationCommand": { "token": "tok" } }
            }})
        };
        let music = parse_chip(&chip("Music")).unwrap();
        assert_eq!(music.title, "Music");
        assert_eq!(music.token.as_deref(), Some("tok"));
        assert!(parse_chip(&chip("Mixes")).is_some());
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
}
