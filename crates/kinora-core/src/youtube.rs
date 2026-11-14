//! Small YouTube helpers shared by Home, the account and playback.

use std::path::PathBuf;

use pcore::{Channel, ChannelId, VideoId};
use rustypipe::client::RustyPipe;
use tokio::sync::OnceCell;
use url::Url;

use crate::Error;

/// `pcore`'s platform name for YouTube.
pub const PLATFORM: &str = "youtube";

/// Whether a video is on YouTube (only those have Home, Up next and prepared playback).
pub fn is_youtube(id: &VideoId) -> bool {
    id.platform == PLATFORM
}

/// The process' RustyPipe client. Its cache (the solved signature challenge,
/// the signed-in session) lives in `storage_dir`; only the first call's
/// directory is used.
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

pub(crate) fn channel(id: &str, name: &str) -> Channel {
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

pub(crate) fn watch_url(id: &str) -> Url {
    Url::parse_with_params("https://youtube.com/watch", &[("v", id)])
        .expect("YouTube URL parseable with video ID")
}

/// The last non-empty line of a command's output, e.g. its error message.
pub(crate) fn last_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .last()
        .map(str::to_owned)
}
