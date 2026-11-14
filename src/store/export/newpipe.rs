use std::borrow::Cow;

use gdk::gio::{
    File, FileCreateFlags,
    prelude::{FileExt, IOStreamExt, OutputStreamExt, SeekableExt},
};
use pcore::Channel;
use url::Url;

use crate::store::import::{NewPipeBase, NewPipeSubscription};

const APP_VERSION: &str = "0.27.7";
const APP_VERSION_INT: u64 = 1004;

pub fn to_newpipe_json(channels: &[Channel], file: File) -> Result<(), Box<dyn std::error::Error>> {
    let subscriptions = channels
        .iter()
        .filter_map(|c| {
            match c.id.platform.as_ref() {
                "youtube" => Some(NewPipeSubscription {
                    service_id: 0,
                    // TODO: Error when failing to parse.
                    url: Url::parse(&format!("https://www.youtube.com/channel/{}", c.id.id))
                        .ok()?,
                    name: c.name.clone(),
                }),
                "peertube" => Some(NewPipeSubscription {
                    service_id: 3,
                    // TODO: Error when failing to parse.
                    url: Url::parse(&format!(
                        "https://{}/video-channels/{}",
                        c.id.id.split_once('@')?.1,
                        c.id.id.split_once('@')?.0,
                    ))
                    .ok()?,
                    name: c.name.clone(),
                }),
                // TODO: Other platforms.
                _ => None,
            }
        })
        .collect();

    let base = NewPipeBase {
        app_version: Cow::Borrowed(APP_VERSION),
        app_version_int: APP_VERSION_INT,
        subscriptions,
    };

    let json = serde_json::to_vec(&base)?;

    if !file.query_exists(None::<&gdk::gio::Cancellable>) {
        // TODO: Async
        file.create(
            FileCreateFlags::PRIVATE | FileCreateFlags::REPLACE_DESTINATION,
            None::<&gdk::gio::Cancellable>,
        )?;
    }
    let io = file.open_readwrite(None::<&gdk::gio::Cancellable>)?;
    io.truncate(0, None::<&gdk::gio::Cancellable>)?;
    let output = io.output_stream();
    output.write(&json, None::<&gdk::gio::Cancellable>)?;

    Ok(())
}
