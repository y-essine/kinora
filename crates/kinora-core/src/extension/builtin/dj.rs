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
/// Setting (number 0..12): the crossfade in seconds; 0 is off.
pub const CROSSFADE_SECONDS: &str = "crossfade-seconds";
/// Setting (toggle): never play a song twice in a session.
pub const AVOID_REPEATS: &str = "avoid-repeats";
/// Setting (toggle): play the tracks' videos instead of audio only.
pub const SHOW_VIDEO: &str = "show-video";
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
                    Setting {
                        key: CROSSFADE_SECONDS.to_owned(),
                        title: "Crossfade".to_owned(),
                        subtitle: Some(
                            "How long one track fades into the next. Short tracks fade shorter"
                                .to_owned(),
                        ),
                        kind: SettingKind::Number {
                            min: 0,
                            max: MAX_CROSSFADE.as_secs() as u32,
                            default: crate::dj::DEFAULT_CROSSFADE.as_secs() as u32,
                            unit: Some("s".to_owned()),
                            min_label: Some("Off".to_owned()),
                        },
                    },
                    Setting::toggle(
                        AVOID_REPEATS,
                        "Avoid Repeats",
                        Some("Never play a song twice in a session, in any version"),
                        true,
                    ),
                    Setting::toggle(
                        SHOW_VIDEO,
                        "Show Video",
                        Some("Off, the DJ plays audio only: tracks start faster and use less data"),
                        false,
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
        show_video: ctx.flag(SHOW_VIDEO),
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
        let kind = &manifest.setting(CROSSFADE_SECONDS).unwrap().kind;
        let SettingKind::Number {
            min, max, default, ..
        } = kind
        else {
            panic!("crossfade is a number");
        };
        assert_eq!((*min, *max, *default), (0, 12, 6));
        assert!(kind.accepts(&SettingValue::Text("12".to_owned())));
        assert!(!kind.accepts(&SettingValue::Text("13".to_owned())));
        assert!(!kind.accepts(&SettingValue::Text("six".to_owned())));
        assert_eq!(kind.default_value(), SettingValue::Text("6".to_owned()));
    }
}
