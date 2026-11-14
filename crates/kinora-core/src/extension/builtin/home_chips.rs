//! Home's filter chips ("All", "Music", "Gaming", ...) as an extension. Off:
//! Home is a single feed without a chip bar (see [`crate::Core::home`]).

use std::sync::Arc;

use crate::HomePage;
use crate::extension::{Context, Extension, FeedFilter, Manifest, Setting};

pub const ID: &str = "home-chips";
/// Setting (off by default): leave Mixes and Playlists out of Home, both the
/// chips whose feeds hold only playlists and the playlist cards
/// ([`HomePage::playlists`]).
pub const HIDE_PLAYLIST_CHIPS: &str = "hide-playlist-chips";

/// Chips whose feeds hold only playlists and mixes, no plain videos.
/// (Requests ask for English, so the titles are stable.)
pub const PLAYLIST_ONLY_CHIPS: &[&str] = &["Mixes", "Playlists"];

pub struct HomeChipsExtension {
    manifest: Manifest,
}

impl Default for HomeChipsExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl HomeChipsExtension {
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                id: ID.to_owned(),
                name: "Home Filters".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                description: "Topic rows on Home, like Music or Gaming".to_owned(),
                permissions: Vec::new(),
                settings: vec![Setting::toggle(
                    HIDE_PLAYLIST_CHIPS,
                    "Hide Mixes and Playlists",
                    Some("Only videos on Home, no playlists to play with the DJ"),
                    false,
                )],
                builtin: true,
            },
        }
    }
}

impl Extension for HomeChipsExtension {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn feed_filters(&self) -> Vec<Arc<dyn FeedFilter>> {
        vec![Arc::new(PlaylistChips)]
    }
}

struct PlaylistChips;

impl FeedFilter for PlaylistChips {
    fn filter(&self, ctx: &Context, page: &mut HomePage) {
        if ctx.flag(HIDE_PLAYLIST_CHIPS) {
            page.chips
                .retain(|c| !PLAYLIST_ONLY_CHIPS.contains(&c.title.as_str()));
            page.playlists.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::extension::{ExtensionRegistry, SettingValue, value_key};
    use crate::{AccountConfig, HomeChip, Paths, PlaylistCard, PlaylistKind};

    fn page() -> HomePage {
        let chip = |title: &str, token: Option<&str>| HomeChip {
            title: title.to_owned(),
            token: token.map(str::to_owned),
        };
        HomePage {
            chips: vec![
                chip("All", None),
                chip("Mixes", Some("a")),
                chip("Music", Some("b")),
                chip("Playlists", Some("c")),
            ],
            playlists: vec![PlaylistCard {
                id: "RDMM".to_owned(),
                title: "My Mix".to_owned(),
                kind: PlaylistKind::Mix,
                thumbnail: None,
                owner: None,
                owner_id: None,
                video_count: None,
                first_video_id: None,
            }],
            ..Default::default()
        }
    }

    fn titles(page: &HomePage) -> Vec<&str> {
        page.chips.iter().map(|c| c.title.as_str()).collect()
    }

    #[test]
    fn playlists_are_kept_unless_hidden() {
        let registry = ExtensionRegistry::with_builtins();
        let (account, paths) = (AccountConfig::anonymous(), Paths::new("/tmp/kinora-test"));

        // By default the playlist chips and cards stay: the DJ plays them.
        let mut home = page();
        registry.filter_feed(&mut home, &account, &paths);
        assert_eq!(titles(&home), ["All", "Mixes", "Music", "Playlists"]);
        assert_eq!(home.playlists.len(), 1);

        // "Hide Mixes and Playlists" drops both.
        registry.set_values(HashMap::from([(
            value_key(ID, HIDE_PLAYLIST_CHIPS),
            SettingValue::Bool(true),
        )]));
        let mut home = page();
        registry.filter_feed(&mut home, &account, &paths);
        assert_eq!(titles(&home), ["All", "Music"]);
        assert!(home.playlists.is_empty());
    }

    #[test]
    fn the_setting_is_off_by_default() {
        let ext = HomeChipsExtension::new();
        let setting = ext.manifest().setting(HIDE_PLAYLIST_CHIPS).unwrap();
        assert_eq!(setting.title, "Hide Mixes and Playlists");
        assert_eq!(
            setting.kind,
            crate::extension::SettingKind::Toggle { default: false }
        );
    }
}
