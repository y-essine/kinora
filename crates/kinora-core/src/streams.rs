//! Kinora: getting videos ready to play quickly.
//!
//! **Prepared playback** (`prepare_with_ytdlp`): for the video you're most
//! likely to watch next, run `kinora-prepare` in the background — the same
//! extraction Clapper's yt-dlp enhancer would do at play time, written to a
//! manifest file. Playing that file starts in 1-3 s instead of ~10 s.
//!
//! **Experimental fast start** (`playback_uri`, setting `fast-start`): gets a YouTube video's streams with RustyPipe, which
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
use std::collections::HashSet;
use std::sync::Mutex;

use crate::{Error, youtube};

/// Prepared playback URIs, reused until shortly before YouTube's links expire.
static PREPARED: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);
/// Videos being prepared right now, so a video is prepared only once at a time.
static PREPARING: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// How long a prepared file is reused (YouTube's links last about six hours).
const PREPARED_TTL: Duration = Duration::from_secs(60 * 60);
/// At most this many videos are prepared at the same time.
const MAX_PREPARING: usize = 2;

/// A URI Clapper can play for this YouTube video: a local DASH manifest, or an
/// HLS URL for live streams.
pub async fn playback_uri(
    storage_dir: PathBuf,
    cache_dir: PathBuf,
    video_id: String,
) -> Result<String, Error> {
    if let Some(uri) = prepared(&video_id) {
        return Ok(uri);
    }
    let rp = youtube::rusty_pipe(storage_dir).await?;
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
    remember(video_id, uri.clone(), lifetime);
    Ok(uri)
}

/// A prepared playback URI for this video, if there is a fresh one.
pub fn prepared(video_id: &str) -> Option<String> {
    let mut guard = PREPARED.lock().unwrap();
    let map = guard.as_mut()?;
    map.retain(|_, (_, until)| *until > Instant::now());
    map.get(video_id).map(|(uri, _)| uri.clone())
}

fn remember(video_id: String, uri: String, lifetime: Duration) {
    PREPARED
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(video_id, (uri, Instant::now() + lifetime));
}

/// Prepare a YouTube video in the background with `kinora-prepare` (see the
/// module docs). Does nothing if it is already prepared or being prepared.
pub async fn prepare_with_ytdlp(cache_dir: PathBuf, video_id: String) -> Result<(), Error> {
    prepare_ytdlp(cache_dir, video_id, false).await
}

/// Like [`prepare_with_ytdlp`], audio only (original language): starts in
/// about half the time and decodes no video. [`prepared_audio`] has it.
pub async fn prepare_audio_with_ytdlp(cache_dir: PathBuf, video_id: String) -> Result<(), Error> {
    prepare_ytdlp(cache_dir, video_id, true).await
}

/// A prepared audio-only playback URI for this video, if there is a fresh one.
pub fn prepared_audio(video_id: &str) -> Option<String> {
    prepared(&audio_key(video_id))
}

/// Audio-only preparations share the map under their own key.
fn audio_key(video_id: &str) -> String {
    format!("{video_id}#audio")
}

async fn prepare_ytdlp(cache_dir: PathBuf, video_id: String, audio: bool) -> Result<(), Error> {
    let key = if audio { audio_key(&video_id) } else { video_id.clone() };
    if prepared(&key).is_some() {
        return Ok(());
    }
    {
        let mut guard = PREPARING.lock().unwrap();
        let preparing = guard.get_or_insert_with(HashSet::new);
        // At most two at a time: sweeping the pointer over a grid shouldn't
        // start a yt-dlp run per video.
        if preparing.len() >= MAX_PREPARING || !preparing.insert(key.clone()) {
            return Ok(());
        }
    }
    let result = async {
        std::fs::create_dir_all(&cache_dir)?;
        let started = Instant::now();
        let mut command = tokio::process::Command::new("kinora-prepare");
        if audio {
            command.arg("--audio");
        }
        let out = command
            .arg(format!("https://www.youtube.com/watch?v={video_id}"))
            .arg(cache_dir.join(&video_id))
            .output()
            .await?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err::<(), Error>(
                err.lines()
                    .last()
                    .unwrap_or("kinora-prepare failed")
                    .to_owned()
                    .into(),
            );
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let uri = url::Url::from_file_path(&path)
            .map_err(|_| format!("not an absolute path: {path}"))?
            .to_string();
        log::info!(
            "Prepared {video_id}{} in {:.1} s",
            if audio { " (audio)" } else { "" },
            started.elapsed().as_secs_f64()
        );
        remember(key.clone(), uri, PREPARED_TTL);
        Ok(())
    }
    .await;
    if let Some(set) = PREPARING.lock().unwrap().as_mut() {
        set.remove(&key);
    }
    result
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

#[cfg(test)]
mod audio_key_tests {
    #[test]
    fn audio_and_video_are_kept_apart() {
        super::remember("abc".into(), "file:///v.m3u8".into(), std::time::Duration::from_secs(60));
        assert_eq!(super::prepared_audio("abc"), None);
        super::remember(super::audio_key("abc"), "file:///a.m3u8".into(), std::time::Duration::from_secs(60));
        assert_eq!(super::prepared_audio("abc").as_deref(), Some("file:///a.m3u8"));
        assert_eq!(super::prepared("abc").as_deref(), Some("file:///v.m3u8"));
    }
}
