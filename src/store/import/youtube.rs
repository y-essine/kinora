use csv::ReaderBuilder;
use gdk::{
    gio::{Cancellable, File},
    prelude::FileExtManual,
};
use pcore::{Channel, ChannelId, Provider};

pub fn from_youtube_csv(file: File) -> Result<Vec<Channel>, Box<dyn std::error::Error>> {
    let content = file.load_contents(Cancellable::NONE)?.0.to_vec();

    let csv_reader = ReaderBuilder::new()
        .has_headers(true)
        .from_reader(&*content);

    let mut result = vec![];

    for record in csv_reader.into_records() {
        let record = record?;
        let id = record[0].to_owned();
        let name = record[2].to_owned();

        let channel = Channel {
            id: ChannelId {
                id,
                platform: ppiped::PipedProvider::platform_id(),
            },
            name,
            avatar: Default::default(),
            banner: Default::default(),
            description: Default::default(),
            subscribers: Default::default(),
        };

        result.push(channel);
    }

    Ok(result)
}
