use csv::Writer;
use gdk::gio::{
    File, FileCreateFlags,
    prelude::{FileExt, IOStreamExt, OutputStreamExt, SeekableExt},
};
use pcore::Channel;

pub fn to_youtube_csv(channels: &[Channel], file: File) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = Writer::from_writer(vec![]);
    writer.write_record(["Channel Id", "Channel Url", "Channel Title"])?;

    for c in channels {
        if c.id.platform != "youtube" {
            continue;
        }

        writer.write_record([
            &c.id.id,
            &format!("http://www.youtube.com/channel/{}", c.id.id),
            &c.name,
        ])?;
    }

    let data = writer.into_inner()?;

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
    output.write(&data, None::<&gdk::gio::Cancellable>)?;

    Ok(())
}
