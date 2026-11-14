use std::borrow::Cow;

use gdk::{
    gio::{Cancellable, File},
    prelude::FileExtManual,
};
use pcore::{Channel, ChannelId, Provider};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Serialize, Deserialize)]
pub struct NewPipeBase {
    pub app_version: Cow<'static, str>,
    pub app_version_int: u64,
    pub subscriptions: Vec<NewPipeSubscription>,
}

#[derive(Serialize, Deserialize)]
pub struct NewPipeSubscription {
    pub service_id: u32,
    pub url: Url,
    pub name: String,
}

pub fn from_newpipe_json(file: File) -> Result<Vec<Channel>, Box<dyn std::error::Error>> {
    let content = file.load_contents(Cancellable::NONE)?.0.to_vec();

    let deserialized: NewPipeBase = serde_json::from_slice(&content)?;

    let mut result = vec![];
    for subscription in deserialized.subscriptions {
        let channel = match subscription.service_id {
            // YouTube
            0 => {
                if let Some(id) = subscription
                    .url
                    .path_segments()
                    .and_then(|mut i| i.next_back())
                {
                    Some(Channel {
                        id: ChannelId {
                            id: id.to_owned(),
                            platform: ppiped::PipedProvider::platform_id(),
                        },
                        name: subscription.name,
                        avatar: Default::default(),
                        banner: Default::default(),
                        description: Default::default(),
                        subscribers: Default::default(),
                    })
                } else {
                    log::warn!(
                        "Could not parse YouTube subscription ID from URL {}",
                        subscription.url
                    );
                    None
                }
            }
            // PeerTube
            3 => {
                if let (Some(host), Some(id)) = (
                    subscription.url.host_str(),
                    subscription
                        .url
                        .path_segments()
                        .and_then(|mut i| i.next_back()),
                ) {
                    Some(Channel {
                        id: ChannelId {
                            id: format!("{id}@{host}"),
                            platform: ppeertube::PeertubeProvider::platform_id(),
                        },
                        name: subscription.name,
                        avatar: Default::default(),
                        banner: Default::default(),
                        description: Default::default(),
                        subscribers: Default::default(),
                    })
                } else {
                    log::warn!(
                        "Could not parse Peertube subscription ID from URL {}",
                        subscription.url
                    );
                    None
                }
            }
            // Unsupported
            _ => {
                log::warn!(
                    "Unsupported subscription service {} for {} ({})",
                    subscription.service_id,
                    subscription.name,
                    subscription.url
                );
                None
            }
        };

        if let Some(channel) = channel {
            result.push(channel);
        }
    }

    Ok(result)
}
