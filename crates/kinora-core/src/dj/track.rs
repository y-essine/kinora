//! A track of a DJ session: a YouTube video seen as a song.

use std::time::Duration;

use chrono::TimeDelta;
use pcore::{Video, VideoId};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::youtube::{PLATFORM, channel, watch_url};

/// One song of a radio, with what YouTube Music says about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// The YouTube video id (11 characters); play it like any video.
    pub id: String,
    pub title: String,
    /// The performers, main artist first. May be empty (rare), or a channel
    /// name for tracks from the regular YouTube fallback.
    pub artists: Vec<Artist>,
    /// Only for songs (YouTube Music's "audio track" items).
    pub album: Option<Album>,
    /// `None` when YouTube did not say (live streams).
    pub duration: Option<Duration>,
    /// The largest thumbnail: square cover art for songs, 16:9 for videos.
    pub thumbnail: Option<Url>,
    /// The release year, when YouTube Music lists it (songs).
    pub year: Option<u16>,
    pub kind: TrackKind,
}

/// A performer (a YouTube Music artist or a channel).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub name: String,
    /// The artist's channel (`UC...`), if linked.
    pub channel_id: Option<String>,
}

/// The album a song is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
    pub name: String,
    /// YouTube Music's album browse id (`MPREb_...`), if linked.
    pub id: Option<String>,
}

/// What kind of YouTube video a track is. Songs are audio with a cover;
/// the others have a picture worth showing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrackKind {
    /// An "audio track" (`MUSIC_VIDEO_TYPE_ATV`): the record with a cover.
    Song,
    /// An official music video (`MUSIC_VIDEO_TYPE_OMV`).
    MusicVideo,
    /// Any other upload (`MUSIC_VIDEO_TYPE_UGC`, or the YouTube fallback).
    #[default]
    Video,
}

impl TrackKind {
    pub(crate) fn from_music_video_type(t: &str) -> Self {
        match t {
            "MUSIC_VIDEO_TYPE_ATV" => Self::Song,
            "MUSIC_VIDEO_TYPE_OMV" => Self::MusicVideo,
            _ => Self::Video,
        }
    }
}

impl Track {
    /// The main artist's name, or `""`.
    pub fn artist(&self) -> &str {
        self.artists.first().map_or("", |a| a.name.as_str())
    }

    /// "Daft Punk, Pharrell Williams & Nile Rodgers".
    pub fn artists_line(&self) -> String {
        let names: Vec<&str> = self.artists.iter().map(|a| a.name.as_str()).collect();
        match names.as_slice() {
            [] => String::new(),
            [one] => (*one).to_owned(),
            [rest @ .., last] => format!("{} & {last}", rest.join(", ")),
        }
    }

    pub fn video_id(&self) -> VideoId {
        VideoId {
            id: self.id.clone(),
            platform: PLATFORM.into(),
        }
    }

    /// The track as a `pcore::Video`, for everything that plays or prepares
    /// videos ([`crate::Core::prepare`], the frontend's video models). The
    /// uploader is the main artist; the upload date is unknown (the epoch).
    pub fn to_video(&self) -> Video {
        let artist = self.artists.first();
        Video {
            id: self.video_id(),
            playable_url: watch_url(&self.id),
            uploader: channel(
                artist.and_then(|a| a.channel_id.as_deref()).unwrap_or(""),
                &self.artists_line(),
            ),
            title: self.title.clone(),
            uploaded: chrono::DateTime::UNIX_EPOCH,
            duration: self.duration.and_then(|d| TimeDelta::from_std(d).ok()),
            thumbnail: self.thumbnail.clone(),
            description: None,
            views: None,
            likes: None,
            dislikes: None,
            is_live: Some(self.duration.is_none()),
            requires_payment: Some(false),
        }
    }

    /// A track for a video the frontend already has (the DJ's seed, or a
    /// "play next" pick from elsewhere in the app). The channel stands in for
    /// the artist.
    pub fn from_video(video: &Video) -> Self {
        let channel = &video.uploader;
        Self {
            id: video.id.id.clone(),
            title: video.title.clone(),
            artists: (!channel.name.is_empty())
                .then(|| Artist {
                    name: clean_channel_name(&channel.name),
                    channel_id: Some(channel.id.id.clone()).filter(|id| !id.is_empty()),
                })
                .into_iter()
                .collect(),
            album: None,
            duration: video.duration.and_then(|d| d.to_std().ok()),
            thumbnail: video.thumbnail.clone(),
            year: None,
            kind: TrackKind::Video,
        }
    }
}

/// Plain single spaces: YouTube's titles hold no-break spaces and doubled
/// spaces now and then.
pub(crate) fn tidy(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "Daft Punk - Topic" and "DaftPunkVEVO" are Daft Punk.
pub(crate) fn clean_channel_name(name: &str) -> String {
    let name = name.trim();
    let name = name.strip_suffix(" - Topic").unwrap_or(name);
    let name = name.strip_suffix("VEVO").unwrap_or(name);
    name.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artist(name: &str) -> Artist {
        Artist {
            name: name.to_owned(),
            channel_id: None,
        }
    }

    #[test]
    fn artists_are_joined_like_youtube_music() {
        let mut track = Track {
            id: "abcdefghijk".into(),
            title: "Get Lucky".into(),
            artists: vec![],
            album: None,
            duration: Some(Duration::from_secs(249)),
            thumbnail: None,
            year: None,
            kind: TrackKind::Song,
        };
        assert_eq!(track.artists_line(), "");
        track.artists = vec![artist("Daft Punk")];
        assert_eq!(track.artists_line(), "Daft Punk");
        track.artists.push(artist("Pharrell Williams"));
        track.artists.push(artist("Nile Rodgers"));
        assert_eq!(
            track.artists_line(),
            "Daft Punk, Pharrell Williams & Nile Rodgers"
        );

        let video = track.to_video();
        assert_eq!(video.id.id, "abcdefghijk");
        assert_eq!(video.duration, Some(TimeDelta::seconds(249)));
        assert_eq!(video.is_live, Some(false));
        let back = Track::from_video(&video);
        assert_eq!(back.duration, track.duration);
        assert_eq!(back.artist(), "Daft Punk, Pharrell Williams & Nile Rodgers");
    }

    #[test]
    fn channel_names_lose_topic_and_vevo() {
        assert_eq!(clean_channel_name("Daft Punk - Topic"), "Daft Punk");
        assert_eq!(clean_channel_name("RickAstleyVEVO"), "RickAstley");
        assert_eq!(clean_channel_name("Rick Astley"), "Rick Astley");
    }
}
