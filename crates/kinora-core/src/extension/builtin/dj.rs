//! The DJ as an extension: its manifest, settings and permissions. The
//! engine is [`crate::dj`]; frontends use it through [`crate::Core`]'s
//! `dj_*` methods, which work only while this extension is enabled.
//!
//! It contributes no hooks: a DJ session is started on purpose, and the
//! frontend drives it (ordinary autoplay is not taken over).

use std::time::Duration;

use crate::dj::{DjSettings, MAX_CROSSFADE, StartFrom};
use crate::extension::{
    Context, Extension, Manifest, Permission, Setting, SettingChoice, SettingKind,
};

pub const ID: &str = "dj";
/// Setting (choice "0".."12"): the crossfade in seconds; "0" is off.
pub const CROSSFADE_SECONDS: &str = "crossfade-seconds";
/// Setting (toggle): never play a song twice in a session.
pub const AVOID_REPEATS: &str = "avoid-repeats";
/// Setting (choice): where "DJ" starts, [`CURRENT_VIDEO`] or [`MY_MIX`].
pub const START_FROM: &str = "start-from";
pub const CURRENT_VIDEO: &str = "current-video";
pub const MY_MIX: &str = "my-mix";

pub struct DjExtension {
    manifest: Manifest,
}

impl Default for DjExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl DjExtension {
    pub fn new() -> Self {
        let seconds = (0..=MAX_CROSSFADE.as_secs())
            .map(|s| SettingChoice {
                value: s.to_string(),
                label: match s {
                    0 => "Off".to_owned(),
                    1 => "1 second".to_owned(),
                    s => format!("{s} seconds"),
                },
            })
            .collect();
        let choice = |key: &str, title: &str, subtitle: &str, options, default: &str| Setting {
            key: key.to_owned(),
            title: title.to_owned(),
            subtitle: Some(subtitle.to_owned()),
            kind: SettingKind::Choice {
                options,
                default: default.to_owned(),
            },
        };
        let option = |value: &str, label: &str| SettingChoice {
            value: value.to_owned(),
            label: label.to_owned(),
        };
        Self {
            manifest: Manifest {
                id: ID.to_owned(),
                name: "DJ".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                description: "Endless music from a song or your mix: YouTube Music's radio, \
                    no repeats, and crossfades between tracks"
                    .to_owned(),
                // The account is optional: with it, the radio and "my mix" are personal.
                permissions: vec![
                    Permission::Network,
                    Permission::Account,
                    Permission::PlayerControl,
                ],
                settings: vec![
                    choice(
                        CROSSFADE_SECONDS,
                        "Crossfade",
                        "How long one track fades into the next. Short tracks fade shorter",
                        seconds,
                        "6",
                    ),
                    Setting::toggle(
                        AVOID_REPEATS,
                        "Avoid Repeats",
                        Some("Never play a song twice in a session, in any version"),
                        true,
                    ),
                    choice(
                        START_FROM,
                        "Start From",
                        "What the DJ plays when you start it",
                        vec![
                            option(CURRENT_VIDEO, "Current Video"),
                            option(MY_MIX, "My Mix"),
                        ],
                        CURRENT_VIDEO,
                    ),
                ],
                builtin: true,
            },
        }
    }
}

impl Extension for DjExtension {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

/// The DJ's settings out of its context (defaults for anything unset).
pub fn settings(ctx: &Context) -> DjSettings {
    let seconds = ctx
        .text(CROSSFADE_SECONDS)
        .parse::<u64>()
        .map_or(crate::dj::DEFAULT_CROSSFADE, Duration::from_secs);
    DjSettings {
        crossfade: seconds.min(MAX_CROSSFADE),
        avoid_repeats: ctx.flag(AVOID_REPEATS),
        start_from: match ctx.text(START_FROM) {
            MY_MIX => StartFrom::MyMix,
            _ => StartFrom::CurrentVideo,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::extension::{ExtensionRegistry, SettingValue, value_key};
    use crate::{AccountConfig, Paths};

    #[test]
    fn settings_have_defaults_and_follow_the_stored_values() {
        let registry = ExtensionRegistry::with_builtins();
        let (account, paths) = (AccountConfig::anonymous(), Paths::new("/tmp/kinora-test"));
        let ctx = || registry.context(ID, &account, &paths).unwrap();
        assert_eq!(settings(&ctx()), DjSettings::default());

        let text = |s: &str| SettingValue::Text(s.to_owned());
        registry.set_values(HashMap::from([
            (value_key(ID, CROSSFADE_SECONDS), text("0")),
            (value_key(ID, AVOID_REPEATS), SettingValue::Bool(false)),
            (value_key(ID, START_FROM), text(MY_MIX)),
        ]));
        let s = settings(&ctx());
        assert_eq!(s.crossfade, Duration::ZERO);
        assert!(!s.avoid_repeats);
        assert_eq!(s.start_from, StartFrom::MyMix);

        // Out of range: the default.
        registry.set_values(HashMap::from([(
            value_key(ID, CROSSFADE_SECONDS),
            text("40"),
        )]));
        assert_eq!(settings(&ctx()).crossfade, crate::dj::DEFAULT_CROSSFADE);
    }

    #[test]
    fn the_manifest_offers_twelve_seconds_at_most() {
        let dj = DjExtension::new();
        let manifest = dj.manifest();
        manifest.validate().unwrap();
        assert_eq!(manifest.name, "DJ");
        assert!(manifest.has(Permission::PlayerControl));
        let SettingKind::Choice { options, default } =
            &manifest.setting(CROSSFADE_SECONDS).unwrap().kind
        else {
            panic!("crossfade is a choice");
        };
        assert_eq!(options.len(), 13);
        assert_eq!(options[0].label, "Off");
        assert_eq!(options.last().unwrap().value, "12");
        assert_eq!(default, "6");
    }
}
