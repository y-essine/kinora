//! Playlists and mixes: what Home's playlist cards, the account's playlists
//! and the DJ's playlist seeds share.
//!
//! A [`PlaylistCard`] is a listing entry (a title, a thumbnail, a count), not
//! the playlist's videos: to play one, start a DJ session from it
//! ([`PlaylistCard::dj_seed`] → [`crate::Core::dj_start`]).
//!
//! Playlist ids, by prefix:
//!
//! | Prefix | What | [`PlaylistKind`] |
//! |---|---|---|
//! | `PL…` | a user's or channel's playlist | `Playlist` |
//! | `OLAK5uy_…` | an album (YouTube Music) | `Album` |
//! | `RD…` | a mix (`RDMM` "my mix", `RD<videoId>`, `RDEM…` artist mixes, …): endless | `Mix` |
//! | `RDCLAK5uy_…` | a YouTube Music curated playlist (finite, despite `RD`) | `Playlist` |
//! | `LL`, `LM`, `WL` | the account's liked videos, liked music, watch later | `Playlist` |
//! | `VL…` | a playlist's browse id: the id after `VL` ([`normalize_id`] strips it) | |

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::dj::DjSeed;
use crate::youtube::{best_url, parse_count};

/// The account's "Liked videos" (YouTube).
pub const LIKED_VIDEOS: &str = "LL";
/// The account's "Liked music" (YouTube Music).
pub const LIKED_MUSIC: &str = "LM";
/// The account's "Watch later".
pub const WATCH_LATER: &str = "WL";

/// What a playlist is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlaylistKind {
    /// Made by YouTube from a video, an artist or the account's taste;
    /// endless (it plays like a radio).
    Mix,
    /// An ordinary, finite playlist.
    #[default]
    Playlist,
    /// An album (`OLAK5uy_…`), in track order.
    Album,
}

impl PlaylistKind {
    /// The kind a playlist id says (see the module's table).
    pub fn of(id: &str) -> Self {
        let id = normalize_id(id).unwrap_or_default();
        if id.starts_with("OLAK5uy_") {
            Self::Album
        } else if id.starts_with("RD") && !id.starts_with("RDCLAK") {
            Self::Mix
        } else {
            Self::Playlist
        }
    }
}

/// A playlist or a mix as a listing shows it (Home, the account's
/// playlists). Draw it as a card; play it with [`Self::dj_seed`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistCard {
    /// The playlist id (`PL…`, `RD…`, `OLAK5uy_…`, `LL`, …), without a
    /// leading `VL`.
    pub id: String,
    pub title: String,
    pub kind: PlaylistKind,
    /// The cover: the first video's thumbnail, or the playlist's own.
    pub thumbnail: Option<Url>,
    /// Who made it: the channel's name, "YouTube" for some mixes, or for
    /// others the artists in it ("Daft Punk, Thomas Bangalter, and more").
    pub owner: Option<String>,
    /// The owner's channel id (`UC…`) when the listing links one.
    pub owner_id: Option<String>,
    /// How many videos, when the listing says ("24 videos"); `None` for
    /// mixes (endless) and when unknown.
    pub video_count: Option<u32>,
    /// The video the card opens on (the first one, or the video a mix is
    /// made from), when the listing says.
    pub first_video_id: Option<String>,
}

impl PlaylistCard {
    /// A DJ seed that plays this playlist (in order, or shuffled), or this
    /// mix (endless, `shuffle` is ignored).
    pub fn dj_seed(&self, shuffle: bool) -> DjSeed {
        DjSeed::Playlist {
            id: self.id.clone(),
            title: Some(self.title.clone()).filter(|t| !t.is_empty()),
            shuffle: shuffle && self.kind != PlaylistKind::Mix,
        }
    }
}

/// A playlist id out of what a user or a listing gives: the id itself, a
/// browse id (`VLPL…`), or a URL with `list=` (`…/playlist?list=PL…`,
/// `…/watch?v=…&list=RD…`). `None` when nothing id-like is left.
pub fn normalize_id(input: &str) -> Option<String> {
    let input = input.trim();
    let id = match input.find("list=") {
        Some(at) => input[at + 5..].split(['&', '#']).next().unwrap_or_default(),
        None => input,
    };
    let id = match id.strip_prefix("VL") {
        Some(rest) if !rest.is_empty() => rest,
        _ => id,
    };
    let valid = id.len() >= 2
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    valid.then(|| id.to_owned())
}

/// A playlist card out of a `lockupViewModel` (Home's grid, search, channel
/// pages). `None` for videos, podcasts and anything without an id or title.
pub(crate) fn from_lockup(l: &Value) -> Option<PlaylistCard> {
    if !matches!(
        l["contentType"].as_str(),
        Some("LOCKUP_CONTENT_TYPE_PLAYLIST" | "LOCKUP_CONTENT_TYPE_ALBUM")
    ) {
        return None;
    }
    let id = normalize_id(l["contentId"].as_str()?)?;
    let meta = &l["metadata"]["lockupMetadataViewModel"];
    let title = meta["title"]["content"].as_str()?.trim().to_owned();
    if title.is_empty() {
        return None;
    }

    // The first row is the byline: "Owner • Playlist".
    let byline = meta["metadata"]["contentMetadataViewModel"]["metadataRows"][0]["metadataParts"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let owner_part = byline.iter().find(|part| {
        part["text"]["content"].as_str().is_some_and(|t| {
            !t.trim().is_empty() && !matches!(t.trim(), "Playlist" | "Mix" | "Album" | "Podcast")
        })
    });
    let owner = owner_part.and_then(|p| p["text"]["content"].as_str().map(|t| t.trim().to_owned()));
    let owner_id = owner_part
        .and_then(|p| p["text"]["commandRuns"].as_array())
        .and_then(|runs| {
            runs.iter().find_map(|run| {
                run["onTap"]["innertubeCommand"]["browseEndpoint"]["browseId"].as_str()
            })
        })
        .filter(|id| id.starts_with("UC"))
        .map(str::to_owned);

    let image = &l["contentImage"];
    let thumbnail_view = if image["collectionThumbnailViewModel"].is_object() {
        &image["collectionThumbnailViewModel"]["primaryThumbnail"]["thumbnailViewModel"]
    } else {
        &image["thumbnailViewModel"]
    };
    let mut kind = PlaylistKind::of(&id);
    let mut video_count = None;
    let mut badges = Vec::new();
    collect_badge_texts(&thumbnail_view["overlays"], &mut badges);
    for text in badges {
        if text.eq_ignore_ascii_case("mix") {
            kind = PlaylistKind::Mix;
        } else if text.contains("video") || text.contains("song") || text.contains("track") {
            video_count = parse_count(&text).and_then(|n| u32::try_from(n).ok());
        }
    }
    if kind == PlaylistKind::Mix {
        video_count = None;
    }

    let first_video_id = l["rendererContext"]["commandContext"]["onTap"]["innertubeCommand"]
        ["watchEndpoint"]["videoId"]
        .as_str()
        .map(str::to_owned);
    Some(PlaylistCard {
        id,
        title,
        kind,
        thumbnail: best_url(&thumbnail_view["image"]["sources"]),
        owner,
        owner_id,
        video_count,
        first_video_id,
    })
}

/// The text of every thumbnail badge below `value` ("24 videos", "Mix").
fn collect_badge_texts(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(text) = map
                .get("thumbnailBadgeViewModel")
                .and_then(|b| b["text"].as_str())
            {
                out.push(text.to_owned());
            }
            map.values().for_each(|v| collect_badge_texts(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| collect_badge_texts(v, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real anonymous search lockups (Daft Punk playlists, mixes and an
    /// album), stripped of tracking data and playback URLs.
    fn lockups() -> Vec<Value> {
        let json: Value =
            serde_json::from_str(include_str!("fixtures/playlist_lockups.json")).unwrap();
        json["lockups"].as_array().unwrap().clone()
    }

    #[test]
    fn ids_are_normalized() {
        let id = |s: &str| normalize_id(s);
        assert_eq!(
            id("PLdXJrX9OsbOVq2TKvUc3xGUp2Rm8nTUVh").as_deref(),
            Some("PLdXJrX9OsbOVq2TKvUc3xGUp2Rm8nTUVh")
        );
        assert_eq!(id(" VLPLabc_-1 ").as_deref(), Some("PLabc_-1"));
        assert_eq!(id("VLLL").as_deref(), Some("LL"));
        assert_eq!(
            id("https://www.youtube.com/playlist?list=PLabc&si=xyz").as_deref(),
            Some("PLabc")
        );
        assert_eq!(
            id("https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RDAMVMdQw4w9WgXcQ").as_deref(),
            Some("RDAMVMdQw4w9WgXcQ")
        );
        assert_eq!(
            id("OLAK5uy_lMA_iEf3aqk5YSDsnrPKojXegOiecSF94").as_deref(),
            Some("OLAK5uy_lMA_iEf3aqk5YSDsnrPKojXegOiecSF94")
        );
        assert_eq!(id("WL").as_deref(), Some("WL"));
        assert_eq!(id(""), None);
        assert_eq!(id("not an id"), None);
        assert_eq!(id("list="), None);
    }

    #[test]
    fn kinds_follow_the_id() {
        assert_eq!(PlaylistKind::of("PLabc"), PlaylistKind::Playlist);
        assert_eq!(PlaylistKind::of("VLPLabc"), PlaylistKind::Playlist);
        assert_eq!(PlaylistKind::of("RDMM"), PlaylistKind::Mix);
        assert_eq!(PlaylistKind::of("RDdQw4w9WgXcQ"), PlaylistKind::Mix);
        assert_eq!(
            PlaylistKind::of("RDEMni3jl65KF37F9JYsJM8DGg"),
            PlaylistKind::Mix
        );
        assert_eq!(PlaylistKind::of("RDCLAK5uy_abc"), PlaylistKind::Playlist);
        assert_eq!(PlaylistKind::of("OLAK5uy_abc"), PlaylistKind::Album);
        assert_eq!(PlaylistKind::of(LIKED_VIDEOS), PlaylistKind::Playlist);
    }

    #[test]
    fn playlist_lockups_become_cards() {
        let cards: Vec<PlaylistCard> = lockups().iter().filter_map(from_lockup).collect();
        assert_eq!(
            cards.len(),
            lockups().len() - 1,
            "the video lockup is not a card"
        );

        let best = cards
            .iter()
            .find(|c| c.id == "PLdXJrX9OsbOVq2TKvUc3xGUp2Rm8nTUVh")
            .unwrap();
        assert_eq!(best.title, "Best Of Daft Punk");
        assert_eq!(best.kind, PlaylistKind::Playlist);
        assert_eq!(best.owner.as_deref(), Some("misterepicpants"));
        assert_eq!(best.owner_id.as_deref(), Some("UC8TvRAUfEhL4LtWN9o1l60w"));
        assert_eq!(best.video_count, Some(24));
        assert_eq!(best.first_video_id.as_deref(), Some("ByxxFmKKtUg"));
        assert_eq!(
            best.thumbnail.as_ref().unwrap().host_str(),
            Some("i.ytimg.com")
        );

        let mix = cards
            .iter()
            .find(|c| c.id == "RDEMni3jl65KF37F9JYsJM8DGg")
            .unwrap();
        assert_eq!(mix.title, "Mix - Daft Punk");
        assert_eq!(mix.kind, PlaylistKind::Mix);
        assert_eq!(mix.owner.as_deref(), Some("YouTube"));
        assert_eq!(mix.owner_id, None);
        assert_eq!(mix.video_count, None);
        assert_eq!(mix.first_video_id.as_deref(), Some("FGBhQbmPwH8"));

        let video_mix = cards.iter().find(|c| c.id == "RDVeauURD-qLk").unwrap();
        assert_eq!(video_mix.kind, PlaylistKind::Mix);
        assert_eq!(
            video_mix.owner.as_deref(),
            Some("Daft Punk, Thomas Bangalter, and more")
        );

        let album = cards.iter().find(|c| c.id.starts_with("OLAK5uy_")).unwrap();
        assert_eq!(album.title, "Discovery");
        assert_eq!(album.kind, PlaylistKind::Album);
        assert_eq!(album.video_count, Some(14));
        assert_eq!(album.owner.as_deref(), Some("Daft Punk"));
    }

    #[test]
    fn cards_seed_the_dj() {
        let card = PlaylistCard {
            id: "PLabc".into(),
            title: "Road trip".into(),
            kind: PlaylistKind::Playlist,
            thumbnail: None,
            owner: None,
            owner_id: None,
            video_count: Some(3),
            first_video_id: None,
        };
        assert_eq!(
            card.dj_seed(true),
            DjSeed::Playlist {
                id: "PLabc".into(),
                title: Some("Road trip".into()),
                shuffle: true
            }
        );
        let mix = PlaylistCard {
            id: "RDMM".into(),
            kind: PlaylistKind::Mix,
            title: String::new(),
            ..card
        };
        // A mix is endless: it is never shuffled.
        assert_eq!(
            mix.dj_seed(true),
            DjSeed::Playlist {
                id: "RDMM".into(),
                title: None,
                shuffle: false
            }
        );
    }
}
