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

#[cfg(test)]
mod tests {
    use super::*;

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
}
