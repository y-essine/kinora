use std::{
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use gdk::{
    gdk_pixbuf::Pixbuf,
    gio::{
        File, FileCreateFlags,
        prelude::{FileExt, IOStreamExt},
    },
};
use url::Url;

// Two Weeks
const PURGE_TIME: Duration = Duration::from_secs(14 * 24 * 60 * 60);

#[derive(Clone)]
pub struct Cache {
    path: PathBuf,
}

impl Cache {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            path: path.as_ref().to_owned(),
        }
    }

    fn path_for_url(&self, url: &Url) -> PathBuf {
        self.path
            .join(url.to_string().replace(['/', '?', '%', '=', ':', '.'], "_"))
            .with_extension("jpg")
    }

    pub async fn get(&self, url: &Url) -> Option<Pixbuf> {
        let file = File::for_path(self.path_for_url(url));
        Pixbuf::from_stream_future(&file.read_future(glib::Priority::LOW).await.ok()?)
            .await
            .ok()
    }

    pub async fn store(&self, url: &Url, pixbuf: Pixbuf) {
        let file = File::for_path(self.path_for_url(url));
        let _ = file
            .create_future(FileCreateFlags::NONE, glib::Priority::LOW)
            .await;
        let Ok(stream) = file.open_readwrite_future(glib::Priority::LOW).await else {
            return;
        };
        let _ = pixbuf
            .save_to_streamv_future(&stream.output_stream(), "jpeg", &[])
            .await;
    }

    pub async fn get_or_fetch<F: FnOnce(Url) -> Fut, Fut: Future<Output = Option<Pixbuf>>>(
        &self,
        url: Url,
        fetch: F,
    ) -> Option<Pixbuf> {
        if let Some(pixbuf) = self.get(&url).await {
            Some(pixbuf)
        } else if let Some(pixbuf) = fetch(url.clone()).await {
            self.store(&url, pixbuf.clone()).await;
            Some(pixbuf)
        } else {
            None
        }
    }

    pub fn purge_old(&self) {
        let Ok(files) = std::fs::read_dir(&self.path) else {
            return;
        };

        let now = SystemTime::now();

        for file in files {
            let Ok(file) = file else {
                continue;
            };

            let Ok(accessed) = file.metadata().and_then(|m| m.accessed()) else {
                continue;
            };

            if now.duration_since(accessed).is_ok_and(|d| d > PURGE_TIME) {
                let path = file.path();
                let _ = std::fs::remove_file(path);
            }
        }
    }
}
