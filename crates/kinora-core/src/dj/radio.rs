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
//!
//! A playlist seed ([`DjSeed::Playlist`]) goes through the same `next`
//! endpoint with its playlist id: YouTube Music lists a playlist or an
//! album in order, 50 tracks a page with a continuation token (shuffled
//! server-side on request), and a mix (`RD…`) like a radio. The fallback is
//! regular YouTube's playlist panel (200 videos a page, then a token; signed
//! in when there are cookies, so private playlists work), or its mix.

use std::hash::BuildHasher;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use super::dedupe::split_artist_title;
use super::track::{Album, Artist, Track, TrackKind, clean_channel_name, tidy};
use crate::Error;
use crate::playlist::{PlaylistKind, normalize_id};
use crate::youtube::{Auth, WEB_USER_AGENT, best_url, parse_duration, text_of};

const MUSIC_ORIGIN: &str = "https://music.youtube.com";
const MUSIC_NEXT_URL: &str = "https://music.youtube.com/youtubei/v1/next?prettyPrint=false";
/// A YouTube Music web client version that `next` accepts.
const MUSIC_CLIENT_VERSION: &str = "1.20241211.07.00";

const YOUTUBE_ORIGIN: &str = "https://www.youtube.com";
const YOUTUBE_NEXT_URL: &str = "https://www.youtube.com/youtubei/v1/next?prettyPrint=false";

/// YouTube's "My Mix": personal when signed in, popular songs otherwise.
const MY_MIX: &str = "RDMM";
/// YouTube Music's `next` parameters for a shuffled playlist (what
/// music.youtube.com sends for "Shuffle").
const MUSIC_SHUFFLE_PARAMS: &str = "wAEB8gECKAE%3D";

/// Where a DJ session starts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DjSeed {
    /// The radio of one video (a YouTube video id): songs like it.
    Video(String),
    /// The account's mix ("My Mix"). Without an account it is YouTube's
    /// mix of popular songs.
    MyMix,
    /// A playlist (`PL…`, `LL`, `WL`, …), an album (`OLAK5uy_…`) or a mix
    /// (`RD…`), by playlist id. Make one with [`DjSeed::playlist`] (which
    /// cleans the id up) or [`crate::PlaylistCard::dj_seed`].
    ///
    /// A playlist or album plays in its order (or shuffled), and when it
    /// is over the session goes on with the radio of its last track. A mix
    /// is endless, like a radio.
    Playlist {
        id: String,
        /// For showing ("Playing Road trip"); not sent anywhere.
        #[serde(default)]
        title: Option<String>,
        /// Play the playlist in a random order (YouTube Music shuffles the
        /// whole playlist). Ignored for mixes.
        #[serde(default)]
        shuffle: bool,
    },
}

impl DjSeed {
    /// A playlist seed, in order. `id` may be a playlist id, a browse id
    /// (`VLPL…`) or a URL with `list=`; `None` when it holds no playlist id.
    pub fn playlist(id: &str, title: Option<String>) -> Option<Self> {
        Some(Self::Playlist {
            id: normalize_id(id)?,
            title,
            shuffle: false,
        })
    }

    /// The same playlist seed, shuffled or in order. Other seeds are left
    /// as they are.
    pub fn shuffled(self, on: bool) -> Self {
        match self {
            Self::Playlist { id, title, .. } => Self::Playlist {
                id,
                title,
                shuffle: on,
            },
            other => other,
        }
    }

    /// The playlist id of a playlist seed (cleaned up: no `VL`).
    pub fn playlist_id(&self) -> Option<String> {
        match self {
            Self::Playlist { id, .. } => Some(normalize_id(id).unwrap_or_else(|| id.clone())),
            _ => None,
        }
    }

    /// The title a playlist seed was given.
    pub fn title(&self) -> Option<&str> {
        match self {
            Self::Playlist { title, .. } => title.as_deref(),
            _ => None,
        }
    }

    /// Whether the seed's own list never ends (a radio or a mix). A finite
    /// playlist or album ends, and the session then goes on with radios.
    pub fn is_endless(&self) -> bool {
        match self {
            Self::Video(_) | Self::MyMix => true,
            Self::Playlist { id, .. } => PlaylistKind::of(id) == PlaylistKind::Mix,
        }
    }

    fn shuffle(&self) -> bool {
        matches!(self, Self::Playlist { shuffle: true, .. }) && !self.is_endless()
    }

    fn music_playlist(&self) -> String {
        match self {
            Self::Video(id) => format!("RDAMVM{id}"),
            Self::MyMix => MY_MIX.to_owned(),
            Self::Playlist { .. } => self.playlist_id().unwrap_or_default(),
        }
    }

    fn youtube_playlist(&self) -> String {
        match self {
            Self::Video(id) => format!("RD{id}"),
            Self::MyMix => MY_MIX.to_owned(),
            Self::Playlist { .. } => self.playlist_id().unwrap_or_default(),
        }
    }

    fn video(&self) -> Option<&str> {
        match self {
            Self::Video(id) => Some(id),
            Self::MyMix | Self::Playlist { .. } => None,
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
    /// A regular YouTube playlist panel's next page.
    #[serde(rename = "youtube-playlist")]
    YouTubePlaylist {
        playlist: String,
        token: String,
        shuffle: bool,
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
/// cookies), then YouTube's mix (for a playlist seed: YouTube's playlist,
/// signed in, then anonymously); a continuation tries the service it came
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
            if seed.is_endless() {
                match youtube_page(&seed.youtube_playlist(), seed.video()).await {
                    Ok(page) => return Ok(page),
                    Err(e) => note("YouTube mix", e),
                }
            } else {
                let web = web_auth(cookies).await;
                for auth in auths_for(&web) {
                    let playlist = seed.youtube_playlist();
                    match youtube_playlist_page(&playlist, None, seed.shuffle(), auth).await {
                        Ok(page) => return Ok(page),
                        Err(e) => note("YouTube playlist", e),
                    }
                }
            }
        }
        RadioRequest::More(RadioContinuation(Continuation::YouTubePlaylist {
            playlist,
            token,
            shuffle,
        })) => {
            let web = web_auth(cookies).await;
            for auth in auths_for(&web) {
                match youtube_playlist_page(playlist, Some(token), *shuffle, auth).await {
                    Ok(page) => return Ok(page),
                    Err(e) => note("YouTube playlist continuation", e),
                }
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

/// The first page of the fallback alone: YouTube's mix for the seed (or,
/// for a playlist seed, YouTube's playlist), anonymous. [`fetch`] uses it
/// when YouTube Music fails; continue it with [`fetch`] as usual.
pub async fn youtube_mix(seed: &DjSeed) -> Result<RadioPage, Error> {
    if seed.is_endless() {
        youtube_page(&seed.youtube_playlist(), seed.video()).await
    } else {
        youtube_playlist_page(&seed.youtube_playlist(), None, seed.shuffle(), None).await
    }
}

/// The account's sign-in for www.youtube.com, if it has usable cookies.
async fn web_auth(cookies: Option<&Path>) -> Option<Auth> {
    Auth::from_file(cookies?, "www.youtube.com").await.ok()
}

/// Signed in (when possible), then anonymously.
fn auths_for(auth: &Option<Auth>) -> Vec<Option<&Auth>> {
    match auth {
        Some(auth) => vec![Some(auth), None],
        None => vec![None],
    }
}

async fn music_first(seed: &DjSeed, auth: Option<&Auth>) -> Result<RadioPage, Error> {
    let playlist = seed.music_playlist();
    if playlist.is_empty() {
        return Err("The playlist id is empty".into());
    }
    let mut body = json!({
        "playlistId": playlist,
        "isAudioOnly": true,
        "enablePersistentPlaylistPanel": true,
    });
    match seed {
        DjSeed::Video(_) | DjSeed::MyMix => {
            body["tunerSettingValue"] = "AUTOMIX_SETTING_NORMAL".into();
        }
        DjSeed::Playlist { .. } if seed.shuffle() => {
            body["params"] = MUSIC_SHUFFLE_PARAMS.into();
        }
        DjSeed::Playlist { .. } => {}
    }
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

/// A page of a regular YouTube playlist's panel: the first one (from the
/// playlist's start), or the one a `token` points to. 200 videos a page.
async fn youtube_playlist_page(
    playlist: &str,
    token: Option<&str>,
    shuffle: bool,
    auth: Option<&Auth>,
) -> Result<RadioPage, Error> {
    if playlist.is_empty() {
        return Err("The playlist id is empty".into());
    }
    let mut body = match token {
        Some(token) => json!({ "continuation": token }),
        None => json!({ "playlistId": playlist }),
    };
    body["context"] = json!({ "client": {
        "clientName": "WEB",
        "clientVersion": crate::home::WEB_CLIENT_VERSION,
        "hl": "en",
    } });
    let json = post(YOUTUBE_NEXT_URL, YOUTUBE_ORIGIN, body, auth).await?;
    let (mut tracks, next) = parse_youtube_playlist(&json);
    if tracks.is_empty() {
        return Err("YouTube sent a playlist without videos".into());
    }
    if shuffle {
        shuffle_tracks(&mut tracks);
    }
    Ok(RadioPage {
        tracks,
        continuation: next.map(|token| {
            RadioContinuation(Continuation::YouTubePlaylist {
                playlist: playlist.to_owned(),
                token,
                shuffle,
            })
        }),
        source: RadioSource::YouTube,
        personal: auth.is_some(),
    })
}

/// A random order (each page on its own: YouTube's playlist panel cannot
/// shuffle the whole playlist like YouTube Music does).
fn shuffle_tracks(tracks: &mut [Track]) {
    let random = std::collections::hash_map::RandomState::new();
    tracks.sort_by_cached_key(|t| random.hash_one(&t.id));
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

/// The playlist panel of a regular YouTube `next` response: the first page,
/// or a continuation.
fn youtube_panel(json: &Value) -> &Value {
    let first = &json["contents"]["twoColumnWatchNextResults"]["playlist"]["playlist"];
    if first.is_object() {
        first
    } else {
        &json["continuationContents"]["playlistPanelContinuation"]
    }
}

/// The videos of a regular YouTube `next` response's mix (or playlist) panel.
pub(crate) fn parse_youtube(json: &Value) -> Vec<Track> {
    youtube_panel(json)["contents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| youtube_track(item.get("playlistPanelVideoRenderer")?))
        .collect()
}

/// The videos of a regular YouTube playlist panel and the token of its next
/// page (none on the last page). Unavailable videos are left out (YouTube
/// hides most of them already).
pub(crate) fn parse_youtube_playlist(json: &Value) -> (Vec<Track>, Option<String>) {
    let token = youtube_panel(json)["continuations"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|c| c["nextContinuationData"]["continuation"].as_str())
        .map(str::to_owned);
    (parse_youtube(json), token)
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
            // "Billboard's Top 500 Pop Songs All Time" (248 tracks on YouTube
            // Music): the first page.
            "music-playlist" => include_str!("fixtures/music_playlist.json"),
            // "Daft Punk Greatest Hits" (50 tracks, one page), cut to its
            // first 5 tracks and the automix entry that ends it.
            "music-playlist-short" => include_str!("fixtures/music_playlist_short.json"),
            // The same Daft Punk playlist on regular YouTube (one page).
            "youtube-playlist" => include_str!("fixtures/youtube_playlist.json"),
            // The Billboard playlist's last page on regular YouTube.
            "youtube-playlist-more" => include_str!("fixtures/youtube_playlist_more.json"),
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

    #[test]
    fn playlist_seeds_are_cleaned_up() {
        let seed =
            DjSeed::playlist("VLPLdXJrX9OsbOVq2TKvUc3xGUp2Rm8nTUVh", Some("Best".into())).unwrap();
        assert_eq!(
            seed,
            DjSeed::Playlist {
                id: "PLdXJrX9OsbOVq2TKvUc3xGUp2Rm8nTUVh".into(),
                title: Some("Best".into()),
                shuffle: false,
            }
        );
        assert_eq!(seed.music_playlist(), "PLdXJrX9OsbOVq2TKvUc3xGUp2Rm8nTUVh");
        assert_eq!(
            seed.youtube_playlist(),
            "PLdXJrX9OsbOVq2TKvUc3xGUp2Rm8nTUVh"
        );
        assert_eq!(seed.video(), None);
        assert_eq!(seed.title(), Some("Best"));
        assert!(!seed.is_endless());
        assert!(!seed.shuffle());
        assert!(seed.clone().shuffled(true).shuffle());

        let url = "https://www.youtube.com/watch?v=FGBhQbmPwH8&list=RDEMni3jl65KF37F9JYsJM8DGg";
        let mix = DjSeed::playlist(url, None).unwrap().shuffled(true);
        assert_eq!(
            mix.playlist_id().as_deref(),
            Some("RDEMni3jl65KF37F9JYsJM8DGg")
        );
        // Mixes are endless and never shuffled.
        assert!(mix.is_endless());
        assert!(!mix.shuffle());

        let album = DjSeed::playlist("OLAK5uy_lMA_iEf3aqk5YSDsnrPKojXegOiecSF94", None).unwrap();
        assert!(!album.is_endless());
        assert!(DjSeed::playlist("RDCLAK5uy_abc", None).is_some_and(|s| !s.is_endless()));
        assert_eq!(DjSeed::playlist("  ", None), None);
        // Built by hand with a browse id, it is still sent without `VL`.
        let raw = DjSeed::Playlist {
            id: "VLWL".into(),
            title: None,
            shuffle: false,
        };
        assert_eq!(raw.music_playlist(), "WL");
        assert_eq!(DjSeed::MyMix.shuffled(true), DjSeed::MyMix);
    }

    #[test]
    fn seeds_round_trip_through_serde() {
        let seed = DjSeed::playlist("PLabc", Some("Mine".into()))
            .unwrap()
            .shuffled(true);
        let json = serde_json::to_value(&seed).unwrap();
        assert_eq!(
            json,
            json!({ "playlist": { "id": "PLabc", "title": "Mine", "shuffle": true } })
        );
        assert_eq!(serde_json::from_value::<DjSeed>(json).unwrap(), seed);
        // Older or hand-written seeds may leave the options out.
        let bare: DjSeed =
            serde_json::from_value(json!({ "playlist": { "id": "PLabc" } })).unwrap();
        assert_eq!(bare, DjSeed::playlist("PLabc", None).unwrap());
    }

    #[test]
    fn music_playlist_is_in_order_with_a_continuation() {
        let (tracks, token) = parse_music(&fixture("music-playlist"));
        assert_eq!(tracks.len(), 50);
        assert!(token.is_some(), "a 248-track playlist has more pages");
        assert_eq!(tracks[0].id, "6xzN8Nt0Pok");
        assert_eq!(
            tracks[0].title,
            "I Wanna Dance with Somebody (Who Loves Me)"
        );
        assert_eq!(tracks[1].title, "Dancing Queen");
        assert!(
            tracks
                .iter()
                .all(|t| t.id.len() == 11 && !t.artists.is_empty())
        );

        // The last page of a playlist has no token; its automix entry (the
        // playlist's own radio) is not a track.
        let (tracks, token) = parse_music(&fixture("music-playlist-short"));
        assert_eq!(tracks.len(), 5);
        assert_eq!(token, None);
        assert_eq!(tracks[0].id, "osFFpaI_DDs");
    }

    #[test]
    fn youtube_playlist_pages() {
        let first = fixture("youtube-playlist");
        let (tracks, token) = parse_youtube_playlist(&first);
        // 43 videos; "7 unavailable videos are hidden" is not one.
        assert_eq!(tracks.len(), 43);
        assert_eq!(token, None);
        assert_eq!(tracks[0].id, "osFFpaI_DDs");
        assert!(tracks.iter().all(|t| t.kind == TrackKind::Video));

        let (tracks, token) = parse_youtube_playlist(&fixture("youtube-playlist-more"));
        assert_eq!(tracks.len(), 25);
        assert_eq!(token, None);
        assert_eq!(tracks[0].id, "Pdbg44M37Q4");
    }

    #[test]
    fn shuffling_keeps_every_track() {
        let (mut tracks, _) = parse_music(&fixture("music-playlist"));
        let mut ids: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();
        shuffle_tracks(&mut tracks);
        let mut shuffled: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();
        ids.sort();
        shuffled.sort();
        assert_eq!(ids, shuffled);
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

    /// A public playlist and a mix, anonymously:
    /// `cargo test -p kinora-core -- --ignored live_playlist`.
    #[tokio::test]
    #[ignore = "talks to YouTube"]
    async fn live_playlist() {
        let seed = DjSeed::playlist("PLQCXKl33aBijHlP4kjmQNP1TTeBlnKPoc", None).unwrap();
        let page = fetch(&RadioRequest::Start(seed.clone()), None)
            .await
            .unwrap();
        assert_eq!(page.source, RadioSource::Music);
        assert_eq!(page.tracks.len(), 50);
        let more = RadioRequest::More(page.continuation.unwrap());
        assert!(fetch(&more, None).await.unwrap().tracks.len() > 40);

        let web = youtube_mix(&seed).await.unwrap();
        assert!(web.tracks.len() > 100);
        assert!(web.continuation.is_some());

        let mix = DjSeed::playlist("RDEMni3jl65KF37F9JYsJM8DGg", None).unwrap();
        let page = fetch(&RadioRequest::Start(mix), None).await.unwrap();
        assert!(page.tracks.len() >= 20);
        assert!(page.continuation.is_some());
    }
}
