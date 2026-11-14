//! Home's filter chips ("All", "Music", "Gaming", ...) as an extension. Off:
//! Home is a single feed without a chip bar (see [`crate::Core::home`]).

use std::sync::Arc;

use crate::HomePage;
use crate::extension::{Context, Extension, FeedFilter, Manifest, Setting};

pub const ID: &str = "home-chips";
/// Setting: leave out the chips whose feeds hold only playlists.
pub const HIDE_PLAYLIST_CHIPS: &str = "hide-playlist-chips";

/// Chips whose feeds hold only playlists (Mixes). Kinora shows plain videos
/// only, so these would lead to an empty page. (Requests ask for English, so
/// the titles are stable.)
const PLAYLIST_ONLY_CHIPS: &[&str] = &["Mixes", "Playlists"];

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
                description: "Topic chips above Home, like Music or Gaming".to_owned(),
                permissions: Vec::new(),
                settings: vec![Setting::toggle(
                    HIDE_PLAYLIST_CHIPS,
                    "Hide Mixes and Playlists",
                    Some("Their feeds have no plain videos"),
                    true,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::extension::{ExtensionRegistry, SettingValue, value_key};
    use crate::{AccountConfig, HomeChip, Paths};

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
            ..Default::default()
        }
    }

    fn titles(page: &HomePage) -> Vec<&str> {
        page.chips.iter().map(|c| c.title.as_str()).collect()
    }

    #[test]
    fn playlist_only_chips_are_skipped() {
        let registry = ExtensionRegistry::with_builtins();
        let (account, paths) = (AccountConfig::anonymous(), Paths::new("/tmp/kinora-test"));

        let mut home = page();
        registry.filter_feed(&mut home, &account, &paths);
        assert_eq!(titles(&home), ["All", "Music"]);

        // The setting switched off keeps them.
        registry.set_values(HashMap::from([(
            value_key(ID, HIDE_PLAYLIST_CHIPS),
            SettingValue::Bool(false),
        )]));
        let mut home = page();
        registry.filter_feed(&mut home, &account, &paths);
        assert_eq!(titles(&home), ["All", "Mixes", "Music", "Playlists"]);
    }
}
