use std::{ffi::OsStr, process::Stdio};

use chrono::{Datelike, Days, Local, TimeDelta, Utc};
use glib::Object;
use tokio::process::Command;

use crate::{
    backend::{Channel, SearchItem, Video},
    tspawn,
};

pub struct Utility {}

#[gtk::template_callbacks(functions)]
impl Utility {
    #[template_callback]
    fn or(#[rest] values: &[gtk::glib::Value]) -> bool {
        values
            .iter()
            .any(|v| v.get::<bool>().expect("Expected boolean for argument"))
    }

    #[template_callback]
    fn and(#[rest] values: &[gtk::glib::Value]) -> bool {
        values
            .iter()
            .all(|v| v.get::<bool>().expect("Expected boolean for argument"))
    }

    #[template_callback]
    fn not(#[rest] values: &[gtk::glib::Value]) -> bool {
        !values[0]
            .get::<bool>()
            .expect("Expected boolean for argument")
    }

    #[template_callback]
    fn is_none(value: Option<Object>) -> bool {
        value.is_none()
    }

    #[template_callback]
    fn is_some(value: Option<Object>) -> bool {
        value.is_some()
    }

    #[template_callback(function)]
    fn format_platform(platform: &str) -> &str {
        match platform {
            "youtube" => "YouTube",
            "peertube" => "Peertube",
            _ => "",
        }
    }

    #[template_callback(function)]
    fn format_subscribers(subscribers: u64) -> String {
        // GTK does not have any `Option<u64>` and uses `0` instead.
        // We'll use `channel.has-subscribers` in the UI to not display an unavailable sub count.
        if subscribers != 0 {
            gettextrs::ngettext(
                "One Subscriber",
                "{} Subscribers",
                subscribers.try_into().unwrap_or_default(),
            )
            .replace("{}", &Utility::format_number_compact(subscribers))
        } else {
            gettextrs::gettext("No Subscribers")
        }
    }

    #[template_callback(function)]
    fn watch_later_icon_name(is_watch_later: bool) -> &'static str {
        if is_watch_later {
            "bookmark-toggled-symbolic"
        } else {
            "bookmark-untoggled-symbolic"
        }
    }

    #[template_callback(function)]
    fn watch_later_tooltip_text(is_watch_later: bool) -> String {
        if is_watch_later {
            gettextrs::gettext("Remove video from Watch Later")
        } else {
            gettextrs::gettext("Add video to Watch Later")
        }
    }

    pub fn format_decimal(number: f64) -> String {
        if number >= 10.0 || ((number * 10.0) as u64).is_multiple_of(10) {
            format!("{number:.0}")
        } else {
            format!("{number:.1}")
        }
    }

    #[template_callback]
    pub fn format_number_compact(number: u64) -> String {
        if number > 1_000_000_000 {
            // Note to translators: Format numbers compactly. For numbers at least one billion.
            gettextrs::pgettext("number-formatting", "{}B")
                .replace("{}", &Utility::format_decimal((number as f64) / 1e9))
        } else if number > 1_000_000 {
            // Note to translators: Format numbers compactly. For numbers at least one million.
            gettextrs::pgettext("number-formatting", "{}M")
                .replace("{}", &Utility::format_decimal((number as f64) / 1e6))
        } else if number > 1_000 {
            // Note to translators: Format numbers compactly. For numbers at least one thousand.
            gettextrs::pgettext("number-formatting", "{}K")
                .replace("{}", &Utility::format_decimal((number as f64) / 1e3))
        } else {
            number.to_string()
        }
    }

    #[template_callback]
    fn pixbuf_to_paintable(pixbuf: Option<gtk::gdk_pixbuf::Pixbuf>) -> Option<gdk::Texture> {
        pixbuf.as_ref().map(gdk::Texture::for_pixbuf)
    }

    #[template_callback]
    fn icon_name_for_platform(platform: Option<String>) -> Option<String> {
        platform.map(|p| format!("{p}-symbolic"))
    }

    pub fn accessible_description_for_video(video: Video) -> String {
        gettextrs::gettext("{title} uploaded by {channel} on {date}")
            .replace("{title}", &video.title())
            .replace("{channel}", &video.uploader().name())
            .replace("{date}", &video.uploaded_label())
    }

    pub fn accessible_description_for_channel(channel: Channel) -> String {
        // Translators: For accessibility about a channel, e.g. "Channel Name with 100 subscribers".
        gettextrs::gettext("{name} with {subscribers} subscribers")
            .replace("{name}", &channel.name())
            .replace("{subscribers}", &channel.subscribers().to_string())
    }

    pub fn accessible_description_for_search_item(search: SearchItem) -> String {
        if let Some(video) = search.video() {
            Self::accessible_description_for_video(video)
        } else if let Some(channel) = search.channel() {
            Self::accessible_description_for_channel(channel)
        } else {
            "".to_owned()
        }
    }

    pub fn format_date_human(time: &chrono::DateTime<Utc>) -> String {
        let time = time.with_timezone(&Local);
        let date = time.date_naive();
        let today = Utc::now().date_naive();

        if today == date {
            // Translators: formatting of time of day in a human-readable fashion, see https://docs.rs/chrono/latest/chrono/format/strftime/index.html#specifiers
            time.format(&gettextrs::gettext("%H:%M")).to_string()
        } else if today + Days::new(1) == date {
            gettextrs::gettext("Tomorrow")
        } else if today - Days::new(1) == date {
            gettextrs::gettext("Yesterday")
        } else if today.year() == date.year() {
            // Translators: formatting of dates without year in a human-readable fashion, see https://docs.rs/chrono/latest/chrono/format/strftime/index.html#specifiers
            date.format(&gettextrs::gettext("%a, %d. %B")).to_string()
        } else {
            // Translators: formatting of dates with year in a human-readable fashion, see https://docs.rs/chrono/latest/chrono/format/strftime/index.html#specifiers
            date.format(&gettextrs::gettext("%Y-%m-%d")).to_string()
        }
    }

    pub fn format_duration_human(d: TimeDelta) -> String {
        if d.num_hours() > 0 {
            format!(
                "{}:{:0>2}:{:0>2}",
                d.num_hours(),
                (d - TimeDelta::try_hours(d.num_hours()).unwrap_or_default()).num_minutes(),
                (d - TimeDelta::try_minutes(d.num_minutes()).unwrap_or_default()).num_seconds()
            )
        } else {
            format!(
                "{:0>2}:{:0>2}",
                d.num_minutes(),
                (d - TimeDelta::try_minutes(d.num_minutes()).unwrap_or_default()).num_seconds()
            )
        }
    }

    #[template_callback(function)]
    fn format_description(text: Option<&str>) -> Option<String> {
        use regex::Regex;
        use std::sync::LazyLock;
        // Regex from https://stackoverflow.com/questions/1798912/replace-any-urls-within-a-string-of-text-to-clickable-links-with-php/1798941#1798941.
        // Slightly modified to not include an ending "." and others in the URL, and don't replace links already in an <a>.
        static REGEX_CREATE_LINKS: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(\s)(https?://\S+)\b").expect("Valid regex"));
        // Undo escaping of already existing links.
        static REGEX_FIX_LINKS: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"&lt;a href=&quot;(.*?)&quot;&gt;(.*?)&lt;/a&gt;"#).expect("Valid regex")
        });
        text.map(|t| {
            REGEX_FIX_LINKS
                .replace_all(
                    REGEX_CREATE_LINKS
                        .replace_all(
                            glib::markup_escape_text(t).as_ref(),
                            r#"$1<a href="$2">$2</a>"#,
                        )
                        .as_ref(),
                    r#"<a href="$1">$2</a>"#,
                )
                .into_owned()
        })
    }

    #[template_callback(function)]
    fn format_percent(percentage: f64) -> String {
        // Translators: Formatting for percentage labels (e.g. "50%").
        gettextrs::gettext("{}%").replace("{}", &format!("{:.0}", percentage * 100.0))
    }

    pub async fn run_command_on_url<
        S1: AsRef<str>,
        S2: AsRef<OsStr> + std::marker::Send + 'static,
    >(
        command: S1,
        url: S2,
        remove_gsk_renderer_env_variable: bool,
    ) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
        let mut command = command.as_ref().to_string();
        if crate::config::FLATPAK {
            command = format!("flatpak-spawn --host {command}");
        }

        tspawn!(async move {
            let mut player_iter = command.split(' ');
            let program = player_iter.next().expect("Non-empty player to be defined");
            let args: Vec<_> = player_iter.collect();

            let stdout = if log::log_enabled!(log::Level::Debug) {
                Stdio::inherit()
            } else {
                Stdio::null()
            };

            let stderr = if log::log_enabled!(log::Level::Error) {
                Stdio::inherit()
            } else {
                Stdio::null()
            };

            let mut command = Command::new(program);
            command
                .args(args)
                .arg(url)
                .stdout(stdout)
                .stderr(stderr)
                .stdin(Stdio::null());
            if remove_gsk_renderer_env_variable {
                command.env_remove("GSK_RENDERER");
            }
            let child = command.spawn();

            match child {
                Ok(mut child) => {
                    log::debug!("Successfully spawned child program");
                    match child.wait().await {
                        Ok(s) => {
                            if !s.success() {
                                Err(Box::<dyn std::error::Error + Sync + Send>::from(format!(
                                    "The external player did not terminate successfully, code: {}",
                                    s.code().unwrap_or(-1)
                                )))
                            } else {
                                Ok(())
                            }
                        }
                        Err(e) => Err(Box::new(e) as Box<dyn std::error::Error + Sync + Send>),
                    }
                }
                Err(e) => {
                    log::error!("Failed to spawn child program");
                    Err(Box::new(e) as Box<dyn std::error::Error + Sync + Send>)
                }
            }
        })
        .await
        .expect("Failed to spawn tokio")
    }
}
