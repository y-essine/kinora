//! Kinora: fast start. Gets a YouTube video's streams with RustyPipe, which
//! solves YouTube's signature challenge in-process and caches it (about 0.2 s,
//! against ~9 s for yt-dlp starting Python and Node.js for every video), and
//! hands Clapper a DASH manifest so playback starts low and adapts upward.
//! Live streams use YouTube's HLS manifest. Whoever plays the result falls back
//! to the yt-dlp path when this fails.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rustypipe::model::{AudioCodec, AudioStream, VideoCodec, VideoPlayer, VideoStream};
use tokio::sync::Mutex;

use super::account::{self, Error};

/// Prepared playback URIs, reused until shortly before YouTube's links expire.
static PREPARED: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::const_new(None);

/// A URI Clapper can play for this YouTube video: a local DASH manifest, or an
/// HLS URL for live streams.
pub async fn playback_uri(
    storage_dir: PathBuf,
    cache_dir: PathBuf,
    video_id: String,
) -> Result<String, Error> {
    if let Some(uri) = prepared(&video_id).await {
        return Ok(uri);
    }
    let rp = account::rusty_pipe(storage_dir).await?;
    // Anonymous on purpose: YouTube refuses signed-in stream requests that lack
    // an extra anti-bot token.
    let player = rp.query().unauthenticated().player(&video_id).await?;

    let uri = if player.details.is_live {
        player
            .hls_manifest_url
            .clone()
            .ok_or("live stream without an HLS manifest")?
    } else {
        let mpd = dash_manifest(&player).ok_or("no streams Kinora can play")?;
        std::fs::create_dir_all(&cache_dir)?;
        let path = cache_dir.join(format!("{video_id}.mpd"));
        std::fs::write(&path, mpd)?;
        url::Url::from_file_path(&path)
            .map_err(|_| "manifest path is not absolute")?
            .to_string()
    };

    // YouTube's links stay valid for hours; stop reusing them well before that.
    let lifetime = Duration::from_secs(player.expires_in_seconds.saturating_sub(600) as u64);
    PREPARED
        .lock()
        .await
        .get_or_insert_with(HashMap::new)
        .insert(video_id, (uri.clone(), Instant::now() + lifetime));
    Ok(uri)
}

async fn prepared(video_id: &str) -> Option<String> {
    let mut guard = PREPARED.lock().await;
    let map = guard.as_mut()?;
    map.retain(|_, (_, until)| *until > Instant::now());
    map.get(video_id).map(|(uri, _)| uri.clone())
}

/// A DASH manifest of one video family and one audio family that this system
/// can decode: H.264 (hardware decoding on most machines) or else VP9, plus
/// Opus or else AAC, in the original language. `None` if nothing fits.
fn dash_manifest(player: &VideoPlayer) -> Option<String> {
    let playable_video = |codec: VideoCodec| {
        let mut streams: Vec<&VideoStream> = player
            .video_only_streams
            .iter()
            .filter(|s| s.codec == codec && !s.hdr && s.drm_systems.is_empty())
            .filter(|s| s.init_range.is_some() && s.index_range.is_some())
            .collect();
        streams.sort_by_key(|s| s.bitrate);
        streams
    };
    let video = Some(playable_video(VideoCodec::Avc1))
        .filter(|v| !v.is_empty())
        .or_else(|| Some(playable_video(VideoCodec::Vp9)).filter(|v| !v.is_empty()))?;

    let playable_audio = |codec: AudioCodec| {
        let mut streams: Vec<&AudioStream> = player
            .audio_streams
            .iter()
            .filter(|s| s.codec == codec && s.drm_systems.is_empty())
            .filter(|s| s.init_range.is_some() && s.index_range.is_some())
            // Dubbed tracks are extra languages; keep the original one.
            .filter(|s| s.track.as_ref().is_none_or(|t| t.is_default))
            .collect();
        streams.sort_by_key(|s| s.bitrate);
        streams
    };
    let audio = Some(playable_audio(AudioCodec::Opus))
        .filter(|a| !a.is_empty())
        .or_else(|| Some(playable_audio(AudioCodec::Mp4a)).filter(|a| !a.is_empty()))?;

    let mut mpd = String::new();
    let _ = write!(
        mpd,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" profiles="urn:mpeg:dash:profile:isoff-on-demand:2011" type="static" mediaPresentationDuration="PT{}S" minBufferTime="PT1.5S">
<Period>
"#,
        player.details.duration
    );

    let _ = writeln!(
        mpd,
        r#"<AdaptationSet contentType="video" mimeType="{}" subsegmentAlignment="true">"#,
        mime(&video[0].mime)
    );
    for s in &video {
        let _ = writeln!(
            mpd,
            r#"<Representation id="{}" codecs="{}" bandwidth="{}" width="{}" height="{}" frameRate="{}">"#,
            s.itag,
            codecs(&s.mime),
            s.bitrate,
            s.width,
            s.height,
            s.fps
        );
        segment(
            &mut mpd,
            &s.url,
            s.init_range.as_ref(),
            s.index_range.as_ref(),
        );
        let _ = writeln!(mpd, "</Representation>");
    }
    let _ = writeln!(mpd, "</AdaptationSet>");

    let _ = writeln!(
        mpd,
        r#"<AdaptationSet contentType="audio" mimeType="{}" subsegmentAlignment="true">"#,
        mime(&audio[0].mime)
    );
    for s in &audio {
        let _ = writeln!(
            mpd,
            r#"<Representation id="{}" codecs="{}" bandwidth="{}" audioSamplingRate="48000">"#,
            s.itag,
            codecs(&s.mime),
            s.bitrate
        );
        segment(
            &mut mpd,
            &s.url,
            s.init_range.as_ref(),
            s.index_range.as_ref(),
        );
        let _ = writeln!(mpd, "</Representation>");
    }
    let _ = writeln!(mpd, "</AdaptationSet>\n</Period>\n</MPD>");
    Some(mpd)
}

fn segment(
    mpd: &mut String,
    url: &str,
    init: Option<&std::ops::Range<u32>>,
    index: Option<&std::ops::Range<u32>>,
) {
    // YouTube's ranges are inclusive at both ends, as DASH expects.
    let (Some(init), Some(index)) = (init, index) else {
        return;
    };
    let _ = writeln!(
        mpd,
        r#"<BaseURL>{}</BaseURL>
<SegmentBase indexRange="{}-{}"><Initialization range="{}-{}"/></SegmentBase>"#,
        xml_escape(url),
        index.start,
        index.end,
        init.start,
        init.end
    );
}

/// `video/mp4; codecs="avc1.640028"` → `video/mp4`
fn mime(full: &str) -> &str {
    full.split(';').next().unwrap_or(full).trim()
}

/// `video/mp4; codecs="avc1.640028"` → `avc1.640028`
fn codecs(full: &str) -> String {
    full.split("codecs=")
        .nth(1)
        .map(|c| c.trim_matches(|ch| ch == '"' || ch == ' ').to_owned())
        .unwrap_or_default()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
