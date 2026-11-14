use gdk::{
    gio::{Cancellable, File},
    prelude::FileExtManual,
};
use pcore::{Channel, ChannelId, Provider};
use ppiped::PipedProvider;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct InvidiousExport {
    pub subscriptions: Vec<String>,
}

pub fn from_invidious_json(file: File) -> Result<Vec<Channel>, Box<dyn std::error::Error>> {
    let content = file.load_contents(Cancellable::NONE)?.0.to_vec();

    let deserialized: InvidiousExport = serde_json::from_slice(&content)?;

    let result = deserialized
        .subscriptions
        .into_iter()
        .map(|channel_id| Channel {
            id: ChannelId {
                id: channel_id,
                platform: PipedProvider::platform_id(),
            },
            name: String::new(),
            avatar: None,
            banner: None,
            description: None,
            subscribers: None,
        })
        .collect();

    Ok(result)
}
