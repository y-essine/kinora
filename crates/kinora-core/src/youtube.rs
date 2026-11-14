//! Small YouTube helpers shared by Home, the account and playback.

use std::path::{Path, PathBuf};

use chrono::TimeDelta;
use pcore::{Channel, ChannelId, VideoId};
use rustypipe::client::RustyPipe;
use serde_json::Value;
use sha1::{Digest, Sha1};
use tokio::sync::OnceCell;
use url::Url;

use crate::Error;

/// `pcore`'s platform name for YouTube.
pub const PLATFORM: &str = "youtube";

/// The browser the direct InnerTube requests (Home, the DJ's radio) pretend to be.
pub(crate) const WEB_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

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

/// What a signed-in InnerTube request needs from the account's cookies.txt.
pub(crate) struct Auth {
    /// The `Cookie` header: the cookies for one YouTube host.
    cookie: String,
    sapisid: String,
}

impl Auth {
    /// The cookies of `host` (`www.youtube.com`, `music.youtube.com`) out of
    /// a Netscape cookies.txt: those set for all of `youtube.com` and those
    /// set for the host itself.
    pub(crate) fn from_cookies_txt(txt: &str, host: &str) -> Result<Self, Error> {
        let mut pairs = Vec::new();
        let mut sapisid = None;
        for line in txt.lines() {
            // Chromium-style exports mark HttpOnly cookies with this prefix.
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            if line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 7 {
                continue;
            }
            let domain = fields[0].trim_start_matches('.');
            if domain != "youtube.com" && domain != host {
                continue;
            }
            let (name, value) = (fields[5], fields[6].trim_end());
            if name == "SAPISID" {
                sapisid = Some(value.to_owned());
            }
            pairs.push(format!("{name}={value}"));
        }
        let sapisid = sapisid
            .ok_or("The account cookies hold no YouTube login (SAPISID). Connect it again.")?;
        Ok(Self {
            cookie: pairs.join("; "),
            sapisid,
        })
    }

    pub(crate) async fn from_file(cookies: &Path, host: &str) -> Result<Self, Error> {
        Self::from_cookies_txt(&tokio::fs::read_to_string(cookies).await?, host)
    }

    /// Add the headers of a signed-in request from `origin` (e.g.
    /// `https://music.youtube.com`).
    pub(crate) fn sign(
        &self,
        request: reqwest::RequestBuilder,
        origin: &str,
    ) -> reqwest::RequestBuilder {
        request
            .header("Cookie", &self.cookie)
            .header("Authorization", sapisidhash(&self.sapisid, origin))
            .header("Origin", origin)
            .header("X-Origin", origin)
            .header("X-Goog-AuthUser", "0")
    }
}

/// The `Authorization` header YouTube's web clients send when signed in.
fn sapisidhash(sapisid: &str, origin: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut hash = Sha1::new();
    hash.update(format!("{now} {sapisid} {origin}"));
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("SAPISIDHASH {now}_{hex}")
}

// ---- InnerTube JSON ----

/// Text of `{ "simpleText": ... }` or `{ "runs": [...] }`.
pub(crate) fn text_of(v: &Value) -> Option<String> {
    let text = match v["simpleText"].as_str() {
        Some(t) => t.to_owned(),
        None => v["runs"]
            .as_array()?
            .iter()
            .filter_map(|r| r["text"].as_str())
            .collect(),
    };
    Some(text).filter(|t| !t.is_empty())
}

/// The largest image of a list of `{ "url": ... }` sources (they are sorted by size).
pub(crate) fn best_url(sources: &Value) -> Option<Url> {
    sources
        .as_array()?
        .iter()
        .rev()
        .find_map(|s| s["url"].as_str())
        .and_then(|u| Url::parse(u).ok())
}

/// "12:48" or "1:09:10".
pub(crate) fn parse_duration(text: &str) -> Option<TimeDelta> {
    let mut seconds = 0i64;
    for part in text.trim().split(':') {
        seconds = seconds * 60 + part.parse::<i64>().ok()?;
    }
    Some(TimeDelta::seconds(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_txt_gives_a_header_and_sapisid() {
        let txt = "# Netscape HTTP Cookie File\n\
            .youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tsecret\n\
            #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc\n\
            music.youtube.com\tFALSE\t/\tTRUE\t0\tPREF\tm\n\
            .google.com\tTRUE\t/\tTRUE\t0\tNID\tnope\n";
        let auth = Auth::from_cookies_txt(txt, "www.youtube.com").unwrap();
        assert_eq!(auth.sapisid, "secret");
        assert_eq!(auth.cookie, "SAPISID=secret; SID=abc");
        let music = Auth::from_cookies_txt(txt, "music.youtube.com").unwrap();
        assert_eq!(music.cookie, "SAPISID=secret; SID=abc; PREF=m");
        let google = ".google.com\tTRUE\t/\tTRUE\t0\tNID\tx";
        assert!(Auth::from_cookies_txt(google, "www.youtube.com").is_err());
        assert!(sapisidhash("secret", "https://www.youtube.com").starts_with("SAPISIDHASH "));
    }
}
