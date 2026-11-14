use chrono::{DateTime, TimeDelta, Utc};
use pcore::{Channel, ChannelId, Video, VideoId};
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

use crate::{backend::Filter, store::Error};

use std::path::{Path, PathBuf};

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        Self::from_connection(Connection::open(path)?)
    }

    #[cfg(test)]
    fn in_memory() -> Result<Self, Error> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut connection: Connection) -> Result<Self, Error> {
        Self::migrations().to_latest(&mut connection)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "wal")?;
        Ok(Self { connection })
    }

    pub fn backup<P: AsRef<Path>>(&self, path: P) -> Result<(), Error> {
        // Note: Ensure that the journal mode of the backup is "delete" instead of "wal":
        // If it were in wal-mode, restoring from the backup would create the "-wal" and "-shm" files in the directory where the restored file is contained within.
        // This causes issues in the Flatpak, where Pipeline only has permissions to the sqlite-file itself, but does not have permissions to create the "-wal" and "-shm" file.
        // See https://gitlab.com/schmiddi-on-mobile/pipeline/-/work_items/321.
        let result: String =
            self.connection
                .pragma_update_and_check(None, "journal_mode", "delete", |p| p.get(0))?;
        assert!(&result == "delete");

        self.connection.backup(rusqlite::MAIN_DB, path, None)?;
        self.connection.pragma_update(None, "journal_mode", "wal")?;
        Ok(())
    }

    pub fn restore<P: AsRef<Path>>(&mut self, path: P) -> Result<(), Error> {
        // Passing None to rustqlite restore requires type annotations, which cannot be given.
        fn progress(_: rusqlite::backup::Progress) {}

        self.connection
            .restore(rusqlite::MAIN_DB, path, Some(progress))?;

        self.connection.pragma_update(None, "foreign_keys", "OFF")?;
        Self::migrations().to_latest(&mut self.connection)?;
        self.connection.pragma_update(None, "foreign_keys", "ON")?;

        Ok(())
    }

    fn migrations() -> Migrations<'static> {
        Migrations::new(vec![
            M::up(
                "CREATE TABLE channels (
                    platform TEXT NOT NULL,
                    id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    avatar TEXT,
                    banner TEXT,
                    description TEXT,
                    subscribers INTEGER
                );
                CREATE UNIQUE INDEX idx_channels ON channels (platform, id);",
            )
            .down("DROP TABLE channels; DROP INDEX idx_channels;")
            .comment("Channel Information Table"),
            M::up(
                "CREATE TABLE videos (
                    platform TEXT NOT NULL,
                    id TEXT NOT NULL,
                    playable_url TEXT NOT NULL,
                    uploader_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    uploaded TEXT NOT NULL,
                    duration INTEGER,
                    thumbnail TEXT NOT NULL,
                    description TEXT,
                    views INTEGER,
                    likes INTEGER,
                    dislikes INTEGER,
                    is_live BOOLEAN,
                    FOREIGN KEY (platform, uploader_id) REFERENCES channels (platform, id)
                );
                CREATE UNIQUE INDEX idx_videos ON videos (platform, id);",
            )
            .down("DROP TABLE videos; DROP INDEX idx_videos;")
            .comment("Video Information Table"),
            M::up(
                "CREATE TABLE subscriptions (
                    platform TEXT NOT NULL,
                    channel_id TEXT NOT NULL,
                    FOREIGN KEY (platform, channel_id) REFERENCES channels (platform, id)
                );
                CREATE UNIQUE INDEX idx_subscriptions ON subscriptions (platform, channel_id);",
            )
            .down("DROP TABLE subscriptions; DROP INDEX idx_subscriptions;")
            .comment("Subscription List"),
            M::up(
                "CREATE TABLE watch_later (
                    platform TEXT NOT NULL,
                    video_id TEXT NOT NULL,
                    FOREIGN KEY (platform, video_id) REFERENCES videos (platform, id)
                );
                CREATE UNIQUE INDEX idx_watch_later ON watch_later (platform, video_id);",
            )
            .down("DROP TABLE watch_later; DROP INDEX idx_watch_later;")
            .comment("Watch Later List"),
            M::up(
                "CREATE TABLE filters (
                    platform TEXT,
                    title TEXT,
                    channel TEXT
                ); CREATE UNIQUE INDEX idx_filters ON filters (platform, title, channel);",
            )
            .down("DROP TABLE filters")
            .comment("Filters"),
            M::up(
                "UPDATE channels
                SET platform='youtube'
                WHERE platform='piped';
                UPDATE videos
                SET platform='youtube'
                WHERE platform='piped';
                UPDATE subscriptions
                SET platform='youtube'
                WHERE platform='piped';
                UPDATE watch_later
                SET platform='youtube'
                WHERE platform='piped';",
            )
            .down(
                "UPDATE channels
                SET platform='piped'
                WHERE platform='youtube';
                UPDATE videos
                SET platform='piped'
                WHERE platform='youtube';
                UPDATE subscriptions
                SET platform='piped'
                WHERE platform='youtube';
                UPDATE watch_later
                SET platform='piped'
                WHERE platform='youtube';",
            )
            .comment("Replace `piped` with `youtube` platform"),
            M::up("ALTER TABLE videos ADD requires_payment BOOLEAN;")
                .down("ALTER TABLE videos REMOVE requires_payment;")
                .comment("Added a new 'requires payment' field."),
            M::up(
                "CREATE TABLE watch_history (
                    platform TEXT NOT NULL,
                    video_id TEXT NOT NULL,
                    FOREIGN KEY (platform, video_id) REFERENCES videos (platform, id)
                );
                CREATE UNIQUE INDEX idx_watch_history ON watch_history (platform, video_id);",
            )
            .down("DROP TABLE watch_history; DROP INDEX idx_watch_history;")
            .comment("Watch History"),
            M::up(
                "DROP TABLE watch_history; DROP INDEX IF EXISTS idx_watch_history;
                CREATE TABLE watch_history (
                    platform TEXT NOT NULL,
                    video_id TEXT NOT NULL
                );",
            )
            .down("DROP TABLE watch_history;")
            .comment("Watch History; without foreigh key constraint."),
            M::up(
                "CREATE TABLE downloaded (
                    platform TEXT NOT NULL,
                    video_id TEXT NOT NULL,
                    path TEXT NOT NULL,
                    FOREIGN KEY (platform, video_id) REFERENCES videos (platform, id)
                );
                CREATE UNIQUE INDEX idx_downloaded ON downloaded (platform, video_id);",
            )
            .down("DROP TABLE downloaded; DROP INDEX idx_downloaded;")
            .comment("Downloaded list"),
            M::up(
                "DROP TABLE watch_history;
                CREATE TABLE watch_history (
                    platform TEXT NOT NULL,
                    video_id TEXT NOT NULL,
                    FOREIGN KEY (platform, video_id) REFERENCES videos (platform, id)
                );
                CREATE UNIQUE INDEX idx_watch_history ON watch_history (platform, video_id);",
            )
            .down("DROP TABLE watch_history; DROP INDEX idx_watch_history;")
            .comment("Watch History"),
            M::up("ALTER TABLE watch_history ADD COLUMN watched_at TEXT;")
                .down("ALTER TABLE watch_history DROP COLUMN watched_at;")
                .comment("Watch History watched_at"),
        ])
    }

    pub fn update_channel_if_exists(&mut self, channel: &Channel) -> Result<(), Error> {
        self.connection.execute("UPDATE OR IGNORE channels SET platform = ?1, id = ?2, name = ?3, avatar = ?4, banner = ?5, description = ?6, subscribers = ?7 WHERE platform = ?1 AND id = ?2;", (&channel.id.platform, &channel.id.id, &channel.name, &channel.avatar, &channel.banner, &channel.description, channel.subscribers))?;
        Ok(())
    }

    pub fn update_video_if_exists(&mut self, video: &Video) -> Result<(), Error> {
        let transaction = self.connection.transaction()?;
        let channel = &video.uploader;
        transaction.execute("UPDATE OR IGNORE channels SET platform = ?1, id = ?2, name = ?3, avatar = ?4, banner = ?5, description = ?6, subscribers = ?7 WHERE platform = ?1 AND id = ?2;", (&channel.id.platform, &channel.id.id, &channel.name, &channel.avatar, &channel.banner, &channel.description, channel.subscribers))?;
        transaction.execute("UPDATE OR IGNORE videos SET platform = ?1, id = ?2, playable_url = ?3, uploader_id = ?4, title = ?5, uploaded = ?6, duration = ?7, thumbnail = ?8, description = ?9, views = ?10, likes = ?11, dislikes = ?12, is_live = ?13, requires_payment = ?14) WHERE platform = ?1 AND id = ?2);", (&video.id.platform, &video.id.id, &video.playable_url, &video.uploader.id.id, &video.title, &video.uploaded, video.duration.map(|d| d.num_seconds()), &video.thumbnail, &video.description, video.views, video.likes, video.dislikes, video.is_live, video.requires_payment))?;
        transaction.commit()?;
        Ok(())
    }

    pub fn subscribe(&mut self, channel: &Channel) -> Result<(), Error> {
        let transaction = self.connection.transaction()?;
        transaction.execute("INSERT OR REPLACE INTO channels (platform, id, name, avatar, banner, description, subscribers) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);", (&channel.id.platform, &channel.id.id, &channel.name, &channel.avatar, &channel.banner, &channel.description, channel.subscribers))?;
        transaction.execute(
            "INSERT OR IGNORE INTO subscriptions (platform, channel_id) VALUES (?1, ?2)",
            (&channel.id.platform, &channel.id.id),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn unsubscribe(&mut self, channel: &Channel) -> Result<(), Error> {
        self.connection.execute(
            "DELETE FROM subscriptions WHERE platform = ? AND channel_id = ?;",
            (&channel.id.platform, &channel.id.id),
        )?;
        Ok(())
    }

    pub fn subscriptions(&self) -> Result<Vec<Channel>, Error> {
        let mut statement = self.connection.prepare("SELECT channels.platform, id, name, avatar, banner, description, subscribers FROM channels JOIN subscriptions ON channels.platform = subscriptions.platform AND channels.id = subscriptions.channel_id;")?;
        let channels = statement.query_map([], |row| {
            Ok(Channel {
                id: ChannelId {
                    id: row.get(1)?,
                    platform: std::borrow::Cow::Owned(row.get(0)?),
                },
                name: row.get(2)?,
                avatar: row.get(3)?,
                banner: row.get(4)?,
                description: row.get(5)?,
                subscribers: row.get(6)?,
            })
        })?;

        Ok(channels.collect::<Result<_, _>>()?)
    }

    pub fn watch_later(&mut self, video: &Video) -> Result<(), Error> {
        let transaction = self.connection.transaction()?;
        let channel = &video.uploader;
        transaction.execute("INSERT OR REPLACE INTO channels (platform, id, name, avatar, banner, description, subscribers) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);", (&channel.id.platform, &channel.id.id, &channel.name, &channel.avatar, &channel.banner, &channel.description, channel.subscribers))?;
        transaction.execute("INSERT OR REPLACE INTO videos (platform, id, playable_url, uploader_id, title, uploaded, duration, thumbnail, description, views, likes, dislikes, is_live, requires_payment) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14);", (&video.id.platform, &video.id.id, &video.playable_url, &video.uploader.id.id, &video.title, &video.uploaded, video.duration.map(|d| d.num_seconds()), &video.thumbnail, &video.description, video.views, video.likes, video.dislikes, video.is_live, video.requires_payment))?;
        transaction.execute(
            "INSERT OR IGNORE INTO watch_later (platform, video_id) VALUES (?1, ?2)",
            (&video.id.platform, &video.id.id),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn remove_watch_later(&mut self, video: &Video) -> Result<(), Error> {
        self.connection.execute(
            "DELETE FROM watch_later WHERE platform = ? AND video_id = ?;",
            (&video.id.platform, &video.id.id),
        )?;
        Ok(())
    }

    pub fn watch_later_list(&self) -> Result<Vec<Video>, Error> {
        let mut statement = self.connection.prepare("SELECT videos.platform, videos.id, playable_url, title, uploaded, duration, thumbnail, videos.description, views, likes, dislikes, is_live, requires_payment, channels.id, channels.name, avatar, banner, channels.description, subscribers FROM videos JOIN watch_later ON videos.platform = watch_later.platform AND videos.id = watch_later.video_id JOIN channels on channels.platform = videos.platform AND channels.id = videos.uploader_id ORDER BY uploaded DESC;")?;
        let videos = statement.query_map([], |row| {
            Ok(Video {
                id: VideoId {
                    id: row.get(1)?,
                    platform: std::borrow::Cow::Owned(row.get(0)?),
                },
                playable_url: row.get(2)?,
                uploader: Channel {
                    id: ChannelId {
                        id: row.get(13)?,
                        platform: std::borrow::Cow::Owned(row.get(0)?),
                    },
                    name: row.get(14)?,
                    avatar: row.get(15)?,
                    banner: row.get(16)?,
                    description: row.get(17)?,
                    subscribers: row.get(18)?,
                },
                title: row.get(3)?,
                uploaded: row.get(4)?,
                duration: row.get::<usize, Option<i64>>(5)?.map(TimeDelta::seconds),
                thumbnail: row.get(6)?,
                description: row.get(7)?,
                views: row.get(8)?,
                likes: row.get(9)?,
                dislikes: row.get(10)?,
                is_live: row.get(11)?,
                requires_payment: row.get(12)?,
            })
        })?;

        Ok(videos.collect::<Result<_, _>>()?)
    }

    /// watched adds video to the watch history.
    pub fn watched(&mut self, video: &Video, watched_at: &DateTime<Utc>) -> Result<(), Error> {
        let transaction = self.connection.transaction()?;
        let channel = &video.uploader;
        transaction.execute("INSERT OR REPLACE INTO channels (platform, id, name, avatar, banner, description, subscribers) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);", (&channel.id.platform, &channel.id.id, &channel.name, &channel.avatar, &channel.banner, &channel.description, channel.subscribers))?;
        transaction.execute("INSERT OR REPLACE INTO videos (platform, id, playable_url, uploader_id, title, uploaded, duration, thumbnail, description, views, likes, dislikes, is_live, requires_payment) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14);", (&video.id.platform, &video.id.id, &video.playable_url, &video.uploader.id.id, &video.title, &video.uploaded, video.duration.map(|d| d.num_seconds()), &video.thumbnail, &video.description, video.views, video.likes, video.dislikes, video.is_live, video.requires_payment))?;
        transaction.execute(
            "INSERT OR REPLACE INTO watch_history (platform, video_id, watched_at) VALUES (?1, ?2, ?3)",
            (&video.id.platform, &video.id.id, watched_at),
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// unwatched deletes video from the watch history.
    pub fn unwatched(&mut self, video: &Video) -> Result<(), Error> {
        self.connection.execute(
            "DELETE FROM watch_history WHERE platform = ?1 AND video_id = ?2",
            (&video.id.platform, &video.id.id),
        )?;
        Ok(())
    }

    pub fn delete_watch_history(&mut self) -> Result<(), Error> {
        self.connection.execute("DELETE FROM watch_history;", ())?;
        Ok(())
    }

    pub fn watch_history(&self) -> Result<Vec<(Video, DateTime<Utc>)>, Error> {
        let mut statement = self.connection.prepare("SELECT videos.platform, videos.id, playable_url, title, uploaded, duration, thumbnail, videos.description, views, likes, dislikes, is_live, requires_payment, channels.id, channels.name, avatar, banner, channels.description, subscribers, watched_at FROM videos JOIN watch_history ON videos.platform = watch_history.platform AND videos.id = watch_history.video_id JOIN channels on channels.platform = videos.platform AND channels.id = videos.uploader_id ORDER BY uploaded DESC;")?;
        let videos = statement.query_map([], |row| {
            Ok((
                Video {
                    id: VideoId {
                        id: row.get(1)?,
                        platform: std::borrow::Cow::Owned(row.get(0)?),
                    },
                    playable_url: row.get(2)?,
                    uploader: Channel {
                        id: ChannelId {
                            id: row.get(13)?,
                            platform: std::borrow::Cow::Owned(row.get(0)?),
                        },
                        name: row.get(14)?,
                        avatar: row.get(15)?,
                        banner: row.get(16)?,
                        description: row.get(17)?,
                        subscribers: row.get(18)?,
                    },
                    title: row.get(3)?,
                    uploaded: row.get(4)?,
                    duration: row.get::<usize, Option<i64>>(5)?.map(TimeDelta::seconds),
                    thumbnail: row.get(6)?,
                    description: row.get(7)?,
                    views: row.get(8)?,
                    likes: row.get(9)?,
                    dislikes: row.get(10)?,
                    is_live: row.get(11)?,
                    requires_payment: row.get(12)?,
                },
                row.get::<usize, Option<DateTime<Utc>>>(19)?
                    .unwrap_or_default(), // Before migrations, it was allowed to have no timestamp for watched videos; assign a default timestamp to such videos.
            ))
        })?;

        Ok(videos.collect::<Result<_, _>>()?)
    }

    pub fn downloaded(&mut self, video: &VideoId, path: impl AsRef<Path>) -> Result<(), Error> {
        self.connection.execute(
            "INSERT OR REPLACE INTO downloaded (platform, video_id, path) VALUES (?1, ?2, ?3);",
            (&video.platform, &video.id, path.as_ref().to_string_lossy()),
        )?;
        Ok(())
    }

    pub fn remove_downloaded(&mut self, video: &VideoId) -> Result<(), Error> {
        self.connection.execute(
            "DELETE FROM downloaded WHERE platform = ?1 AND video_id = ?2;",
            (&video.platform, &video.id),
        )?;
        Ok(())
    }

    pub fn downloaded_list(&self) -> Result<Vec<(Video, PathBuf)>, Error> {
        let mut statement = self.connection.prepare("SELECT videos.platform, videos.id, playable_url, title, uploaded, duration, thumbnail, videos.description, views, likes, dislikes, is_live, requires_payment, channels.id, channels.name, avatar, banner, channels.description, subscribers, path FROM videos JOIN downloaded ON videos.platform = downloaded.platform AND videos.id = downloaded.video_id JOIN channels on channels.platform = videos.platform AND channels.id = videos.uploader_id;")?;
        let videos = statement.query_map([], |row| {
            Ok((
                Video {
                    id: VideoId {
                        id: row.get(1)?,
                        platform: std::borrow::Cow::Owned(row.get(0)?),
                    },
                    playable_url: row.get(2)?,
                    uploader: Channel {
                        id: ChannelId {
                            id: row.get(13)?,
                            platform: std::borrow::Cow::Owned(row.get(0)?),
                        },
                        name: row.get(14)?,
                        avatar: row.get(15)?,
                        banner: row.get(16)?,
                        description: row.get(17)?,
                        subscribers: row.get(18)?,
                    },
                    title: row.get(3)?,
                    uploaded: row.get(4)?,
                    duration: row.get::<usize, Option<i64>>(5)?.map(TimeDelta::seconds),
                    thumbnail: row.get(6)?,
                    description: row.get(7)?,
                    views: row.get(8)?,
                    likes: row.get(9)?,
                    dislikes: row.get(10)?,
                    is_live: row.get(11)?,
                    requires_payment: row.get(12)?,
                },
                PathBuf::from(row.get::<usize, String>(19)?),
            ))
        })?;

        Ok(videos.collect::<Result<_, _>>()?)
    }

    pub fn add_filter(&mut self, filter: &Filter) -> Result<(), Error> {
        self.connection.execute(
            "INSERT OR REPLACE INTO filters (platform, title, channel) VALUES (?1, ?2, ?3);",
            (filter.platform(), filter.title(), filter.channel()),
        )?;
        Ok(())
    }

    pub fn remove_filter(&mut self, filter: &Filter) -> Result<(), Error> {
        self.connection.execute(
            "DELETE FROM filters WHERE platform = ?1 AND title = ?2 AND channel = ?3;",
            (filter.platform(), filter.title(), filter.channel()),
        )?;
        Ok(())
    }

    pub fn filters(&self) -> Result<Vec<Filter>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT platform, title, channel FROM filters;")?;
        let channels = statement.query_map([], |row| {
            Ok(Filter::new(
                &row.get::<usize, String>(0)?,
                &row.get::<usize, String>(1)?,
                &row.get::<usize, String>(2)?,
            ))
        })?;

        Ok(channels.collect::<Result<_, _>>()?)
    }

    pub fn trim_unused(&mut self) -> Result<(), Error> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM videos WHERE (platform, id) NOT IN ((SELECT platform, video_id AS id FROM watch_later UNION SELECT platform, video_id AS id FROM watch_history UNION SELECT platform, video_id AS id FROM downloaded);",
            [],
        )?;
        transaction.execute("DELETE FROM channels WHERE (platform, id) NOT IN (SELECT platform, channel_id AS id FROM subscriptions UNION SELECT platform, uploader_id AS id FROM videos);", [])?;
        transaction.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use chrono::{DateTime, Utc};
    use url::Url;

    use super::*;

    #[test]
    fn migrations_test() -> Result<(), rusqlite_migration::Error> {
        Store::migrations().validate()
    }

    fn dummy_channel(n: u64) -> Channel {
        Channel {
            id: ChannelId {
                id: format!("id{}", n),
                platform: std::borrow::Cow::Owned(format!("platform{}", n)),
            },
            name: format!("name{}", n),
            avatar: Some(Url::parse(&format!("https://avatar{}.com", n)).unwrap()),
            banner: Some(Url::parse(&format!("https://banner{}.com", n)).unwrap()),
            description: Some(format!("description{}", n)),
            subscribers: Some(n),
        }
    }

    fn dummy_video(n: u64, uploader: Channel) -> Video {
        Video {
            id: VideoId {
                id: format!("id{}", n),
                platform: std::borrow::Cow::Owned(format!("platform{}", n)),
            },
            playable_url: Url::parse(&format!("https://playable{}.com", n)).unwrap(),
            uploader,
            title: format!("title{}", n),
            uploaded: DateTime::<Utc>::from_timestamp(n as i64, 0).unwrap(),
            duration: Some(TimeDelta::seconds(n as i64)),
            thumbnail: Some(Url::parse(&format!("https://thumbnail{}.com", n)).unwrap()),
            description: Some(format!("description{}", n)),
            views: Some(n),
            likes: Some(n),
            dislikes: Some(n),
            is_live: Some(false),
            requires_payment: Some(false),
        }
    }

    #[test]
    fn subscribe_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let channel1 = dummy_channel(1);
        let channel2 = dummy_channel(2);

        let mut store = Store::in_memory()?;

        store.subscribe(&channel1)?;
        assert_eq!(vec![channel1.clone()], store.subscriptions()?);

        store.subscribe(&channel2)?;
        assert_eq!(
            vec![channel1.clone(), channel2.clone()],
            store.subscriptions()?
        );

        store.unsubscribe(&channel2)?;
        assert_eq!(vec![channel1.clone()], store.subscriptions()?);

        store.unsubscribe(&channel1)?;
        assert!(store.subscriptions()?.is_empty());

        Ok(())
    }

    #[test]
    fn subscribe_update() -> Result<(), Box<dyn std::error::Error>> {
        let channel1 = dummy_channel(1);
        let channel1_updated = {
            let mut c = channel1.clone();
            c.name = "UPDATED".to_string();
            c
        };

        let mut store = Store::in_memory()?;

        store.subscribe(&channel1)?;
        store.subscribe(&channel1_updated)?;
        assert_eq!(vec![channel1_updated.clone()], store.subscriptions()?);

        Ok(())
    }

    #[test]
    fn watch_later_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let channel1 = dummy_channel(1);
        let channel2 = dummy_channel(2);
        let video1 = dummy_video(1, channel1.clone());
        let video2 = dummy_video(2, channel2.clone());

        let mut store = Store::in_memory()?;

        store.watch_later(&video1)?;
        assert_eq!(vec![video1.clone()], store.watch_later_list()?);

        store.watch_later(&video2)?;
        let mut list = store.watch_later_list()?;
        // Don't care about the order of the watch-later list.
        list.sort_by_key(|v| v.id.id.clone());
        assert_eq!(vec![video1.clone(), video2.clone()], list);

        store.remove_watch_later(&video2)?;
        assert_eq!(vec![video1.clone()], store.watch_later_list()?);

        store.remove_watch_later(&video1)?;
        assert!(store.watch_later_list()?.is_empty());

        Ok(())
    }

    #[test]
    fn watch_later_update() -> Result<(), Box<dyn std::error::Error>> {
        let channel1 = dummy_channel(1);
        let video1 = dummy_video(1, channel1.clone());
        let video1_updated = {
            let mut v = video1.clone();
            v.title = "UPDATED".to_owned();
            v
        };

        let mut store = Store::in_memory()?;

        store.watch_later(&video1)?;
        store.watch_later(&video1_updated)?;
        assert_eq!(vec![video1_updated.clone()], store.watch_later_list()?);

        Ok(())
    }

    #[test]
    fn trim() -> Result<(), Box<dyn std::error::Error>> {
        let channel1 = dummy_channel(1);
        let channel2 = dummy_channel(2);
        let channel3 = dummy_channel(3);
        let video1 = dummy_video(1, channel1.clone());
        let video2 = dummy_video(2, channel2.clone());

        let mut store = Store::in_memory()?;

        store.subscribe(&channel1)?;
        store.subscribe(&channel3)?;
        store.watch_later(&video1)?;
        store.watch_later(&video2)?;

        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM videos;", [], |row| row
                    .get::<usize, i32>(0))?,
            2
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM channels;", [], |row| row
                    .get::<usize, i32>(0))?,
            3
        );

        store.remove_watch_later(&video2)?;
        store.remove_watch_later(&video1)?;
        store.unsubscribe(&channel1)?;
        store.unsubscribe(&channel3)?;

        store.trim_unused()?;

        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM videos;", [], |row| row
                    .get::<usize, i32>(0))?,
            0
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM channels;", [], |row| row
                    .get::<usize, i32>(0))?,
            0
        );

        Ok(())
    }
}
