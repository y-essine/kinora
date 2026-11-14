//! The DJ's radio: an endless list of songs that go with a seed, from
//! YouTube Music's `next` endpoint (the queue music.youtube.com shows after
//! "Start radio"), with regular YouTube's mix as the fallback.
//!
//! - **YouTube Music** (`WEB_REMIX` client): playlist `RDAMVM<videoId>` for
//!   a video, `RDMM` for "my mix". Anonymous works; with the account's
//!   cookies the radio is personal. Pages of about 50 tracks, continued with
//!   a radio continuation token.
//! - **YouTube** (`WEB` client) when YouTube Music fails or sends nothing:
//!   the mix `RD<videoId>` (or `RDMM`), 25 videos a page. It has no token:
//!   the next page is the mix seen from its last video, which overlaps the
//!   page before (the session's dedupe drops the repeats).

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use super::dedupe::split_artist_title;
use super::track::{Album, Artist, Track, TrackKind, clean_channel_name, tidy};
use crate::Error;
use crate::youtube::{Auth, WEB_USER_AGENT, best_url, parse_duration, text_of};

const MUSIC_ORIGIN: &str = "https://music.youtube.com";
const MUSIC_NEXT_URL: &str = "https://music.youtube.com/youtubei/v1/next?prettyPrint=false";
/// A YouTube Music web client version that `next` accepts.
const MUSIC_CLIENT_VERSION: &str = "1.20241211.07.00";

const YOUTUBE_ORIGIN: &str = "https://www.youtube.com";
const YOUTUBE_NEXT_URL: &str = "https://www.youtube.com/youtubei/v1/next?prettyPrint=false";

/// YouTube's "My Mix": personal when signed in, popular songs otherwise.
const MY_MIX: &str = "RDMM";

/// Where a DJ session starts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DjSeed {
    /// The radio of one video (a YouTube video id): songs like it.
    Video(String),
    /// The account's mix ("My Mix"). Without an account it is YouTube's
    /// mix of popular songs.
    MyMix,
}

impl DjSeed {
    fn music_playlist(&self) -> String {
        match self {
            Self::Video(id) => format!("RDAMVM{id}"),
            Self::MyMix => MY_MIX.to_owned(),
        }
    }

    fn youtube_playlist(&self) -> String {
        match self {
            Self::Video(id) => format!("RD{id}"),
            Self::MyMix => MY_MIX.to_owned(),
        }
    }

    fn video(&self) -> Option<&str> {
        match self {
            Self::Video(id) => Some(id),
            Self::MyMix => None,
        }
    }
}

/// Which service a page came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RadioSource {
    /// YouTube Music's radio (songs with artists and albums).
    Music,
    /// The regular YouTube mix, the fallback (videos; the artist is the
    /// channel or the "Artist - " part of the title).
    #[serde(rename = "youtube")]
    YouTube,
}

/// Where to continue a radio. Opaque: keep it and pass it back in
/// [`RadioRequest::More`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioContinuation(Continuation);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Continuation {
    Music {
        playlist: String,
        token: String,
    },
    #[serde(rename = "youtube")]
    YouTube {
        playlist: String,
        after: String,
    },
}

/// What to fetch: the first page of a radio, or the page after one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RadioRequest {
    Start(DjSeed),
    More(RadioContinuation),
}

/// One page of a radio.
#[derive(Debug, Clone, PartialEq)]
pub struct RadioPage {
    /// In YouTube's order. The first page of a video's radio starts with the
    /// video itself.
    pub tracks: Vec<Track>,
    /// `None`: this radio has no more pages (start a new one from a recent
    /// track; [`super::DjSession::refill_request`] does).
    pub continuation: Option<RadioContinuation>,
    pub source: RadioSource,
    /// Fetched with the account's cookies (the radio is personal).
    pub personal: bool,
}

/// Fetch a page of a radio. `cookies`: the account's cookies.txt, for a
/// personal radio; `None` is anonymous.
///
/// A first page tries YouTube Music signed in, then anonymously (stale
/// cookies), then YouTube's mix; a continuation tries the service it came
/// from (signed in, then anonymously). A page without tracks counts as a
/// failure.
pub async fn fetch(request: &RadioRequest, cookies: Option<&Path>) -> Result<RadioPage, Error> {
    // Bad cookies make the radio anonymous rather than failing.
    let auth = match cookies {
        Some(path) => match Auth::from_file(path, "music.youtube.com").await {
            Ok(auth) => Some(auth),
            Err(e) => {
                log::warn!("DJ: the account's cookies are unusable, going anonymous: {e}");
                None
            }
        },
        None => None,
    };
    let mut auths = vec![auth.as_ref()];
    if auth.is_some() {
        auths.push(None);
    }

    let mut last_error: Option<Error> = None;
    let mut note = |what: &str, e: Error| {
        log::warn!("DJ: {what} failed: {e}");
        last_error = Some(e);
    };
    match request {
        RadioRequest::Start(seed) => {
            for &auth in &auths {
                match music_first(seed, auth).await {
                    Ok(page) => return Ok(page),
                    Err(e) => note("YouTube Music radio", e),
                }
            }
            match youtube_page(&seed.youtube_playlist(), seed.video()).await {
                Ok(page) => return Ok(page),
                Err(e) => note("YouTube mix", e),
            }
        }
        RadioRequest::More(RadioContinuation(Continuation::Music { playlist, token })) => {
            for &auth in &auths {
                match music_more(playlist, token, auth).await {
                    Ok(page) => return Ok(page),
                    Err(e) => note("YouTube Music radio continuation", e),
                }
            }
        }
        RadioRequest::More(RadioContinuation(Continuation::YouTube { playlist, after })) => {
            match youtube_page(playlist, Some(after)).await {
                Ok(page) => return Ok(page),
                Err(e) => note("YouTube mix continuation", e),
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "The radio could not be loaded".into()))
}

/// The first page of the fallback alone: YouTube's mix for the seed,
/// anonymous. [`fetch`] uses it when YouTube Music fails; continue it with
/// [`fetch`] as usual.
pub async fn youtube_mix(seed: &DjSeed) -> Result<RadioPage, Error> {
    youtube_page(&seed.youtube_playlist(), seed.video()).await
}

async fn music_first(seed: &DjSeed, auth: Option<&Auth>) -> Result<RadioPage, Error> {
    let playlist = seed.music_playlist();
    let mut body = json!({
        "playlistId": playlist,
        "isAudioOnly": true,
        "enablePersistentPlaylistPanel": true,
        "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
    });
    if let Some(id) = seed.video() {
        body["videoId"] = id.into();
    }
    let json = music_next(body, auth).await?;
    music_page(&json, playlist, auth.is_some())
}

async fn music_more(playlist: &str, token: &str, auth: Option<&Auth>) -> Result<RadioPage, Error> {
    let body = json!({
        "continuation": token,
        "playlistId": playlist,
        "isAudioOnly": true,
        "enablePersistentPlaylistPanel": true,
    });
    let json = music_next(body, auth).await?;
    music_page(&json, playlist.to_owned(), auth.is_some())
}

fn music_page(json: &Value, playlist: String, personal: bool) -> Result<RadioPage, Error> {
    let (tracks, token) = parse_music(json);
    if tracks.is_empty() {
        return Err("YouTube Music sent a radio without tracks".into());
    }
    Ok(RadioPage {
        tracks,
        continuation: token.map(|token| RadioContinuation(Continuation::Music { playlist, token })),
        source: RadioSource::Music,
        personal,
    })
}

/// A page of YouTube's mix, seen from `video` (the seed, or the last video
/// of the page before). Always anonymous: the account's taste is YouTube
/// Music's job.
async fn youtube_page(playlist: &str, video: Option<&str>) -> Result<RadioPage, Error> {
    let mut body = json!({ "playlistId": playlist });
    if let Some(id) = video {
        body["videoId"] = id.into();
    }
    body["context"] = json!({ "client": {
        "clientName": "WEB",
        "clientVersion": crate::home::WEB_CLIENT_VERSION,
        "hl": "en",
    } });
    let json = post(YOUTUBE_NEXT_URL, YOUTUBE_ORIGIN, body, None).await?;
    let tracks = parse_youtube(&json);
    let last = tracks.last().ok_or("YouTube sent a mix without videos")?;
    Ok(RadioPage {
        continuation: Some(RadioContinuation(Continuation::YouTube {
            playlist: playlist.to_owned(),
            after: last.id.clone(),
        })),
        tracks,
        source: RadioSource::YouTube,
        personal: false,
    })
}

async fn music_next(mut body: Value, auth: Option<&Auth>) -> Result<Value, Error> {
    body["context"] = json!({ "client": {
        "clientName": "WEB_REMIX",
        "clientVersion": MUSIC_CLIENT_VERSION,
        "hl": "en",
    } });
    post(MUSIC_NEXT_URL, MUSIC_ORIGIN, body, auth).await
}

async fn post(url: &str, origin: &str, body: Value, auth: Option<&Auth>) -> Result<Value, Error> {
    static HTTP: OnceCell<reqwest::Client> = OnceCell::const_new();
    let http = HTTP
        .get_or_try_init(|| async {
            reqwest::Client::builder()
                .user_agent(WEB_USER_AGENT)
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(15))
                .build()
        })
        .await?;
    let mut request = http
        .post(url)
        .header("Content-Type", "application/json")
        .body(body.to_string());
    request = match auth {
        Some(auth) => auth.sign(request, origin),
        None => request.header("Origin", origin),
    };
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("YouTube answered the radio request with {status}").into());
    }
    Ok(serde_json::from_slice(&response.bytes().await?)?)
}

// ---- Parsing ----

/// Tracks and the continuation token of a YouTube Music `next` response: a
/// first page (`musicQueueRenderer`) or a continuation
/// (`playlistPanelContinuation`).
pub(crate) fn parse_music(json: &Value) -> (Vec<Track>, Option<String>) {
    let first = &json["contents"]["singleColumnMusicWatchNextResultsRenderer"]["tabbedRenderer"]["watchNextTabbedResultsRenderer"]
        ["tabs"][0]["tabRenderer"]["content"]["musicQueueRenderer"]["content"]["playlistPanelRenderer"];
    let panel = if first.is_object() {
        first
    } else {
        &json["continuationContents"]["playlistPanelContinuation"]
    };

    let tracks = panel["contents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            // Signed in, a song with a music video comes wrapped with its counterpart.
            let renderer = item.get("playlistPanelVideoRenderer").or_else(|| {
                item["playlistPanelVideoWrapperRenderer"]["primaryRenderer"]
                    .get("playlistPanelVideoRenderer")
            })?;
            music_track(renderer)
        })
        .collect();
    let token = panel["continuations"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|c| {
            c["nextRadioContinuationData"]["continuation"]
                .as_str()
                .or(c["nextContinuationData"]["continuation"].as_str())
        })
        .map(str::to_owned);
    (tracks, token)
}

fn music_track(r: &Value) -> Option<Track> {
    if r.get("unplayableText").is_some() {
        return None;
    }
    let id = r["videoId"].as_str()?;
    let title = tidy(&text_of(&r["title"])?);

    // "Artist, Artist & Artist • Album • 2001" for songs,
    // "Artist • 1.8B views • 19M likes" for videos.
    let runs = r["longBylineText"]["runs"]
        .as_array()
        .or(r["shortBylineText"]["runs"].as_array())
        .map_or(&[][..], Vec::as_slice);
    let mut segments: Vec<Vec<&Value>> = vec![Vec::new()];
    for run in runs {
        if run["text"].as_str().is_some_and(|t| t.trim() == "•") {
            segments.push(Vec::new());
        } else if let Some(segment) = segments.last_mut() {
            segment.push(run);
        }
    }

    let browse_id = |run: &Value| {
        run["navigationEndpoint"]["browseEndpoint"]["browseId"]
            .as_str()
            .map(str::to_owned)
    };
    let artists: Vec<Artist> = segments[0]
        .iter()
        .filter_map(|run| {
            let name = tidy(run["text"].as_str()?);
            (!matches!(name.as_str(), "" | "," | "&")).then(|| Artist {
                name,
                channel_id: browse_id(run),
            })
        })
        .collect();
    // Uploads titled "Michael Sembello - Maniac" by Michael Sembello.
    let title = match artists.first() {
        Some(a) => split_artist_title(&title, &a.name).map_or(title.clone(), |(_, t)| t.to_owned()),
        None => title,
    };
    let mut album = None;
    let mut year = None;
    for run in segments.iter().skip(1).flatten() {
        let page_type = run["navigationEndpoint"]["browseEndpoint"]
            ["browseEndpointContextSupportedConfigs"]["browseEndpointContextMusicConfig"]
            ["pageType"]
            .as_str();
        let text = tidy(run["text"].as_str().unwrap_or_default());
        if page_type == Some("MUSIC_PAGE_TYPE_ALBUM") {
            album = Some(Album {
                name: text,
                id: browse_id(run),
            });
        } else if text.len() == 4 && text.bytes().all(|b| b.is_ascii_digit()) {
            year = text.parse().ok();
        }
    }

    let kind = r["navigationEndpoint"]["watchEndpoint"]["watchEndpointMusicSupportedConfigs"]
        ["watchEndpointMusicConfig"]["musicVideoType"]
        .as_str()
        .map(TrackKind::from_music_video_type)
        .unwrap_or_default();
    Some(Track {
        id: id.to_owned(),
        title,
        artists,
        album,
        duration: length(&r["lengthText"]),
        thumbnail: best_url(&r["thumbnail"]["thumbnails"]),
        year,
        kind,
    })
}

/// The videos of a regular YouTube `next` response's mix panel.
pub(crate) fn parse_youtube(json: &Value) -> Vec<Track> {
    json["contents"]["twoColumnWatchNextResults"]["playlist"]["playlist"]["contents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| youtube_track(item.get("playlistPanelVideoRenderer")?))
        .collect()
}

fn youtube_track(r: &Value) -> Option<Track> {
    if r.get("unplayableText").is_some() {
        return None;
    }
    let id = r["videoId"].as_str()?;
    let full_title = tidy(&text_of(&r["title"])?);
    let owner = &r["longBylineText"]["runs"][0];
    let channel = clean_channel_name(&tidy(owner["text"].as_str().unwrap_or_default()));
    let channel_id = owner["navigationEndpoint"]["browseEndpoint"]["browseId"]
        .as_str()
        .map(str::to_owned);
    // "Modern Talking - Cheri Cheri Lady (Official Video)" from the channel
    // "Modern Talking Offiziell" is the song "Cheri Cheri Lady (Official
    // Video)" by Modern Talking.
    let (artist, title) = match split_artist_title(&full_title, &channel) {
        Some((artist, title)) => (artist.to_owned(), title.to_owned()),
        None => (channel, full_title),
    };
    Some(Track {
        id: id.to_owned(),
        title,
        artists: (!artist.is_empty())
            .then_some(Artist {
                name: artist,
                channel_id,
            })
            .into_iter()
            .collect(),
        album: None,
        duration: length(&r["lengthText"]),
        thumbnail: best_url(&r["thumbnail"]["thumbnails"]),
        year: None,
        kind: TrackKind::Video,
    })
}

fn length(text: &Value) -> Option<Duration> {
    text_of(text)
        .and_then(|t| parse_duration(&t))
        .and_then(|d| d.to_std().ok())
        .filter(|d| !d.is_zero())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "music" => include_str!("fixtures/music_radio.json"),
            "music-more" => include_str!("fixtures/music_radio_more.json"),
            "youtube" => include_str!("fixtures/youtube_mix.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn music_radio_has_songs_with_albums() {
        let (tracks, token) = parse_music(&fixture("music"));
        assert_eq!(tracks.len(), 50);
        assert!(token.is_some());

        // The seed comes first.
        let seed = &tracks[0];
        assert_eq!(seed.id, "wU26xVT_vBU");
        assert_eq!(seed.title, "One More Time (Radio Edit)");
        assert_eq!(seed.artist(), "Daft Punk");
        assert!(
            seed.artists[0]
                .channel_id
                .as_deref()
                .unwrap()
                .starts_with("UC")
        );
        let album = seed.album.as_ref().unwrap();
        assert_eq!(album.name, "One More Time");
        assert!(album.id.as_deref().unwrap().starts_with("MPRE"));
        assert_eq!(seed.year, Some(2000));
        assert_eq!(seed.duration, Some(Duration::from_secs(5 * 60 + 21)));
        assert_eq!(seed.kind, TrackKind::Song);
        assert!(seed.thumbnail.is_some());

        // Several artists are separate.
        let lucky = tracks.iter().find(|t| t.id == "Rgrt_8mXrK8").unwrap();
        assert_eq!(
            lucky.artists_line(),
            "Daft Punk, Pharrell Williams & Nile Rodgers"
        );
        assert!(
            tracks
                .iter()
                .all(|t| t.id.len() == 11 && !t.title.is_empty())
        );
        assert!(tracks.iter().all(|t| !t.artists.is_empty()));
        assert!(tracks.iter().all(|t| t.duration.is_some()));
    }

    #[test]
    fn music_continuation_is_parsed() {
        let (tracks, token) = parse_music(&fixture("music-more"));
        assert!(tracks.len() > 20, "{}", tracks.len());
        assert!(token.is_some());
    }

    #[test]
    fn music_videos_have_no_album() {
        let item = json!({ "playlistPanelVideoRenderer": {
            "videoId": "dQw4w9WgXcQ",
            "title": { "runs": [ { "text": "Rick Astley - Never Gonna Give You Up" } ] },
            "longBylineText": { "runs": [
                { "text": "Rick Astley", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCuAXFkgsw1L7xaCfnd5JJOw" } } },
                { "text": " • " }, { "text": "1.8B views" }, { "text": " • " }, { "text": "19M likes" },
            ] },
            "lengthText": { "runs": [ { "text": "3:34" } ] },
            "navigationEndpoint": { "watchEndpoint": { "watchEndpointMusicSupportedConfigs": {
                "watchEndpointMusicConfig": { "musicVideoType": "MUSIC_VIDEO_TYPE_OMV" } } } },
        } });
        let unplayable = json!({ "playlistPanelVideoRenderer": {
            "videoId": "aaaaaaaaaaa", "title": { "simpleText": "Gone" },
            "unplayableText": { "simpleText": "Not available" },
        } });
        let page = json!({ "continuationContents": { "playlistPanelContinuation": {
            "contents": [ item, unplayable ],
        } } });
        let (tracks, token) = parse_music(&page);
        assert_eq!(token, None);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].kind, TrackKind::MusicVideo);
        assert_eq!(tracks[0].album, None);
        assert_eq!(tracks[0].year, None);
        assert_eq!(tracks[0].artist(), "Rick Astley");
        // The artist is not repeated in the title.
        assert_eq!(tracks[0].title, "Never Gonna Give You Up");
    }

    #[test]
    fn youtube_mix_splits_artist_from_title() {
        let tracks = parse_youtube(&fixture("youtube"));
        assert_eq!(tracks.len(), 25);
        let seed = &tracks[0];
        assert_eq!(seed.id, "dQw4w9WgXcQ");
        assert_eq!(seed.artist(), "Rick Astley");
        assert_eq!(
            seed.title,
            "Never Gonna Give You Up (Official Video) (4K Remaster)"
        );
        assert_eq!(seed.kind, TrackKind::Video);
        // "Modern Talking - ..." from "Modern Talking Offiziell".
        let cheri = tracks.iter().find(|t| t.id == "eNvUS-6PTbs").unwrap();
        assert_eq!(cheri.artist(), "Modern Talking");
        assert_eq!(cheri.title, "Cheri Cheri Lady (Official Video)");
        assert!(tracks.iter().all(|t| t.duration.is_some()));
    }

    #[test]
    fn seeds_name_their_playlists() {
        let video = DjSeed::Video("dQw4w9WgXcQ".into());
        assert_eq!(video.music_playlist(), "RDAMVMdQw4w9WgXcQ");
        assert_eq!(video.youtube_playlist(), "RDdQw4w9WgXcQ");
        assert_eq!(DjSeed::MyMix.music_playlist(), "RDMM");
        assert_eq!(DjSeed::MyMix.video(), None);
    }

    /// The real thing, anonymously: `cargo test -p kinora-core -- --ignored live_radio`.
    #[tokio::test]
    #[ignore = "talks to YouTube"]
    async fn live_radio() {
        let start = RadioRequest::Start(DjSeed::Video("dQw4w9WgXcQ".into()));
        let page = fetch(&start, None).await.unwrap();
        assert_eq!(page.source, RadioSource::Music);
        assert!(page.tracks.len() >= 20);
        let more = RadioRequest::More(page.continuation.unwrap());
        assert!(!fetch(&more, None).await.unwrap().tracks.is_empty());

        let web = youtube_page("RDdQw4w9WgXcQ", Some("dQw4w9WgXcQ"))
            .await
            .unwrap();
        assert!(web.tracks.len() >= 10);
    }
}
