//! Plain values a frontend passes into the core: where files go, and the
//! settings the core needs. The core never reads settings itself (the GTK
//! app keeps them in GSettings, another frontend may use a file).

use std::path::{Path, PathBuf};

use crate::Error;

/// Where the core keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    cache_dir: PathBuf,
}

impl Paths {
    /// `cache_dir` is the app's own cache folder, e.g. `~/.cache/kinora`.
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
        }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// RustyPipe's cache: the solved signature challenge and the signed-in session.
    pub fn rustypipe_dir(&self) -> PathBuf {
        self.cache_dir.join("account")
    }

    /// Prepared playback manifests.
    pub fn streams_dir(&self) -> PathBuf {
        self.cache_dir.join("streams")
    }

    /// The tempo and beats of tracks the DJ analysed (see [`crate::beats`]).
    pub fn beats_dir(&self) -> PathBuf {
        self.cache_dir.join("beats")
    }
}

/// The YouTube account, as configured by the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountConfig {
    /// A command that prints the path of the account's Netscape cookies.txt
    /// (the `account-cookies-command` setting). Empty: no account, fully anonymous.
    pub cookies_command: String,
}

impl AccountConfig {
    pub fn new(cookies_command: impl Into<String>) -> Self {
        Self {
            cookies_command: cookies_command.into(),
        }
    }

    /// No account at all: the core then never runs the cookies command.
    pub fn anonymous() -> Self {
        Self::default()
    }

    /// Whether an account is set up (it may still fail to sign in).
    pub fn is_connected(&self) -> bool {
        !self.cookies_command.trim().is_empty()
    }

    /// Path to the account's cookies.txt, or `None` if no account is set up.
    pub async fn cookies(&self) -> Result<Option<PathBuf>, Error> {
        crate::account::cookies_file(&self.cookies_command).await
    }
}

/// How videos are started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackConfig {
    /// Experimental: look streams up with RustyPipe instead of yt-dlp (the
    /// `fast-start` setting).
    pub fast_start: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_below_the_cache_dir() {
        let paths = Paths::new("/tmp/k");
        assert_eq!(paths.rustypipe_dir(), PathBuf::from("/tmp/k/account"));
        assert_eq!(paths.streams_dir(), PathBuf::from("/tmp/k/streams"));
    }

    #[tokio::test]
    async fn anonymous_account_has_no_cookies() {
        let account = AccountConfig::new("   ");
        assert!(!account.is_connected());
        assert!(account.cookies().await.unwrap().is_none());
    }
}
