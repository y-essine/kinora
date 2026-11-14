use std::{ffi::OsStr, process::Stdio};

use chrono::{Datelike, Local, TimeDelta, Utc};
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

    /// Kinora: the "ambient glow" of a picture, as a paintable (see `ambient_texture`).
    #[template_callback]
    fn pixbuf_to_ambient(pixbuf: Option<gtk::gdk_pixbuf::Pixbuf>) -> Option<gdk::Texture> {
        pixbuf.as_ref().and_then(Self::ambient_texture)
    }

    /// Kinora: a picture reduced to soft patches of its colours, to be stretched
    /// large behind the player. It is blurred once, here, at a tiny size, so
    /// showing it costs the same as any small image: no blur runs on screen.
    pub fn ambient_texture(pixbuf: &gtk::gdk_pixbuf::Pixbuf) -> Option<gdk::Texture> {
        use gtk::gdk_pixbuf::InterpType;
        const W: usize = 64;
        const H: usize = 36;
        // Average the picture down to a few colour patches, then back up a little.
        let small = pixbuf
            .scale_simple(16, 9, InterpType::Bilinear)?
            .scale_simple(W as i32, H as i32, InterpType::Bilinear)?;
        let channels = small.n_channels() as usize;
        let stride = small.rowstride() as usize;
        let bytes = small.read_pixel_bytes();
        let mut rgb = vec![[0f32; 3]; W * H];
        for y in 0..H {
            for x in 0..W {
                let p = y * stride + x * channels;
                for c in 0..3 {
                    rgb[y * W + x][c] = bytes[p + c] as f32;
                }
            }
        }
        // Two box blurs (about a Gaussian) remove the patches' edges.
        let radius = 5i32;
        for _ in 0..2 {
            for horizontal in [true, false] {
                let source = rgb.clone();
                for y in 0..H as i32 {
                    for x in 0..W as i32 {
                        let mut sum = [0f32; 3];
                        for d in -radius..=radius {
                            let (sx, sy) = if horizontal {
                                ((x + d).clamp(0, W as i32 - 1), y)
                            } else {
                                (x, (y + d).clamp(0, H as i32 - 1))
                            };
                            let s = source[sy as usize * W + sx as usize];
                            for c in 0..3 {
                                sum[c] += s[c];
                            }
                        }
                        let n = (2 * radius + 1) as f32;
                        rgb[y as usize * W + x as usize] = [sum[0] / n, sum[1] / n, sum[2] / n];
                    }
                }
            }
        }
        // More colour and less light than the picture: a glow must never be
        // bright enough to hurt the contrast of text that lies on it.
        let data: Vec<u8> = rgb
            .iter()
            .flat_map(|p| {
                let luma = 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
                p.map(|c| ((luma + (c - luma) * 1.6) * 0.8).clamp(0.0, 255.0) as u8)
            })
            .collect();
        Some(
            gdk::MemoryTexture::new(
                W as i32,
                H as i32,
                gdk::MemoryFormat::R8g8b8,
                &glib::Bytes::from_owned(data),
                W * 3,
            )
            .into(),
        )
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
        Self::format_date_human_at(time, Utc::now())
    }

    // Kinora: YouTube only reports coarse ages ("4 years ago"), which the backend turns into an exact date with today's month and day.
    // So only show weekdays for the last week and relative wording for anything older, instead of dates that look exact but are not.
    fn format_date_human_at(time: &chrono::DateTime<Utc>, now: chrono::DateTime<Utc>) -> String {
        // The epoch marks "not known yet" (videos from a short listing).
        if time.timestamp() == 0 {
            return String::new();
        }
        let time = time.with_timezone(&Local);
        let date = time.date_naive();
        let today = now.with_timezone(&Local).date_naive();
        let days_ago = (today - date).num_days();

        match days_ago {
            0 => {
                // Translators: formatting of time of day in a human-readable fashion, see https://docs.rs/chrono/latest/chrono/format/strftime/index.html#specifiers
                time.format(&gettextrs::gettext("%H:%M")).to_string()
            }
            -1 => gettextrs::gettext("Tomorrow"),
            1 => gettextrs::gettext("Yesterday"),
            2..=6 => {
                // Translators: formatting of the weekday of a date within the last week, see https://docs.rs/chrono/latest/chrono/format/strftime/index.html#specifiers
                date.format(&gettextrs::gettext("%A")).to_string()
            }
            7..=29 => {
                let weeks = (days_ago / 7) as u32;
                // Translators: Relative date, e.g. "3 weeks ago".
                gettextrs::ngettext("{} week ago", "{} weeks ago", weeks)
                    .replace("{}", &weeks.to_string())
            }
            30..=364 => {
                let months = ((days_ago as f64 / 30.4375) as u32).max(1);
                // Translators: Relative date, e.g. "5 months ago".
                gettextrs::ngettext("{} month ago", "{} months ago", months)
                    .replace("{}", &months.to_string())
            }
            365.. => {
                let years = (days_ago / 365) as u32;
                // Translators: Relative date, e.g. "4 years ago".
                gettextrs::ngettext("{} year ago", "{} years ago", years)
                    .replace("{}", &years.to_string())
            }
            // In the future (e.g. scheduled premieres), where the date is exact.
            _ => {
                if today.year() == date.year() {
                    // Translators: formatting of dates without year in a human-readable fashion, see https://docs.rs/chrono/latest/chrono/format/strftime/index.html#specifiers
                    date.format(&gettextrs::gettext("%a, %d. %B")).to_string()
                } else {
                    // Translators: formatting of dates with year in a human-readable fashion, see https://docs.rs/chrono/latest/chrono/format/strftime/index.html#specifiers
                    date.format(&gettextrs::gettext("%Y-%m-%d")).to_string()
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ago(days: i64) -> String {
        // Midday UTC, so that the local date is the same in (almost) every timezone.
        let now = Utc.with_ymd_and_hms(2026, 11, 14, 12, 0, 0).unwrap();
        Utility::format_date_human_at(&(now - TimeDelta::days(days)), now)
    }

    #[test]
    fn unknown_date_is_empty() {
        let epoch = Utc.timestamp_opt(0, 0).unwrap();
        assert_eq!(Utility::format_date_human(&epoch), "");
    }

    #[test]
    fn recent_dates_use_day_names() {
        assert_eq!(ago(1), "Yesterday");
        // 2 to 6 days ago is a weekday name.
        for days in 2..=6 {
            assert!(ago(days).ends_with("day"), "{}", ago(days));
        }
    }

    #[test]
    fn older_dates_are_relative() {
        assert_eq!(ago(7), "1 week ago");
        assert_eq!(ago(21), "3 weeks ago");
        assert_eq!(ago(29), "4 weeks ago");
        assert_eq!(ago(30), "1 month ago");
        assert_eq!(ago(150), "4 months ago");
        assert_eq!(ago(364), "11 months ago");
        assert_eq!(ago(365), "1 year ago");
        assert_eq!(ago(4 * 365 + 20), "4 years ago");
    }
}
