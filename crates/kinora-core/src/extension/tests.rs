//! The registry and the Core's use of it, with made-up extensions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::future::{BoxFuture, ready};
use pcore::{Video, VideoId};

use super::*;
use crate::player::Transition;
use crate::{AccountConfig, Core, HomeChip, HomePage, Paths};

fn manifest(id: &str, permissions: Vec<Permission>, settings: Vec<Setting>) -> Manifest {
    Manifest {
        id: id.to_owned(),
        name: id.to_owned(),
        version: "1".to_owned(),
        description: String::new(),
        permissions,
        settings,
        builtin: false,
    }
}

fn video(id: &str) -> Video {
    Video {
        id: VideoId {
            id: id.to_owned(),
            platform: "youtube".into(),
        },
        playable_url: crate::youtube::watch_url(id),
        uploader: crate::youtube::channel("UC", "Someone"),
        title: id.to_owned(),
        uploaded: chrono::DateTime::UNIX_EPOCH,
        duration: None,
        thumbnail: None,
        description: None,
        views: None,
        likes: None,
        dislikes: None,
        is_live: Some(false),
        requires_payment: Some(false),
    }
}

/// A "DJ" that plays the last candidate with a crossfade, and a feed with chips.
struct Dj {
    manifest: Manifest,
    watched: Arc<Mutex<Vec<String>>>,
}

impl Dj {
    fn new(permissions: Vec<Permission>) -> Self {
        let choice = SettingKind::Choice {
            options: vec![
                SettingChoice {
                    value: "first".into(),
                    label: "First".into(),
                },
                SettingChoice {
                    value: "last".into(),
                    label: "Last".into(),
                },
            ],
            default: "last".into(),
        };
        let settings = vec![Setting {
            key: "pick".into(),
            title: "Pick".into(),
            subtitle: None,
            kind: choice,
        }];
        Self {
            manifest: manifest("dj", permissions, settings),
            watched: Arc::default(),
        }
    }
}

struct DjHook(Arc<Mutex<Vec<String>>>);

impl PlayerHook for DjHook {
    fn on_watched(&self, ctx: Context, video: Video) -> BoxFuture<'static, Result<(), Error>> {
        let seen = self.0.clone();
        Box::pin(async move {
            // The DJ has no Account permission, so it must not get the account.
            assert!(ctx.account().is_err());
            seen.lock().unwrap().push(video.id.id);
            Ok(())
        })
    }

    fn choose_next(&self, ctx: &Context, _current: &Video, candidates: &[Video]) -> Option<usize> {
        match ctx.text("pick") {
            "last" => candidates.len().checked_sub(1),
            _ => Some(0),
        }
    }

    fn transition(&self, _: &Context, _: &Video, _: &Video) -> Option<Transition> {
        Some(Transition::Crossfade(std::time::Duration::from_secs(4)))
    }
}

struct ChipFeed;

impl FeedSource for ChipFeed {
    fn id(&self) -> &str {
        "chips"
    }
    fn title(&self) -> &str {
        "Chips"
    }
    fn placement(&self) -> FeedPlacement {
        FeedPlacement::Main
    }
    fn load(
        &self,
        _: Context,
        request: FeedRequest,
    ) -> BoxFuture<'static, Result<HomePage, Error>> {
        let chip = |t: &str| HomeChip {
            title: t.to_owned(),
            token: Some(t.to_owned()),
        };
        Box::pin(ready(Ok(HomePage {
            videos: vec![video(request.chip.as_deref().unwrap_or("all"))],
            chips: vec![chip("Music"), chip("Mixes")],
            next: None,
        })))
    }
}

impl Extension for Dj {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn player_hooks(&self) -> Vec<Arc<dyn PlayerHook>> {
        vec![Arc::new(DjHook(self.watched.clone()))]
    }
    fn feed_sources(&self) -> Vec<Arc<dyn FeedSource>> {
        vec![Arc::new(ChipFeed)]
    }
}

fn env() -> (AccountConfig, Paths) {
    (AccountConfig::anonymous(), Paths::new("/tmp/kinora-test"))
}

#[test]
fn ids_must_be_valid_and_unique() {
    struct Plain(Manifest);
    impl Extension for Plain {
        fn manifest(&self) -> &Manifest {
            &self.0
        }
    }
    let mut registry = ExtensionRegistry::with_builtins();
    let taken = Plain(manifest("account", vec![], vec![]));
    assert!(registry.register(Arc::new(taken)).is_err());
    for bad in ["", "Has Caps", "-x", "a.b"] {
        let plain = Plain(manifest(bad, vec![], vec![]));
        assert!(registry.register(Arc::new(plain)).is_err(), "{bad:?}");
    }
    let twice = vec![
        Setting::toggle("a", "A", None, true),
        Setting::toggle("a", "A", None, false),
    ];
    assert!(
        registry
            .register(Arc::new(Plain(manifest("x", vec![], twice))))
            .is_err()
    );
    assert!(
        registry
            .register(Arc::new(Plain(manifest("fine-1", vec![], vec![]))))
            .is_ok()
    );
}

#[test]
fn disabled_list_switches_extensions_off() {
    let registry = ExtensionRegistry::with_builtins();
    assert!(registry.is_enabled("account"));
    assert!(registry.is_enabled("home-chips"));
    assert!(!registry.is_enabled("not-installed"));

    registry.set_disabled(["home-chips", "not-installed"]);
    assert!(registry.is_enabled("account"));
    assert!(!registry.is_enabled("home-chips"));

    assert_eq!(
        registry.set_enabled("account", false),
        ["account", "home-chips", "not-installed"]
    );
    assert_eq!(
        registry.set_enabled("home-chips", true),
        ["account", "not-installed"]
    );

    let panels = registry.settings_panels();
    assert_eq!(panels.len(), 2);
    assert!(!panels[0].enabled && panels[0].builtin);
    assert_eq!(panels[0].values["mark-watched"], SettingValue::Bool(true));
}

#[test]
fn stored_values_that_do_not_fit_are_ignored() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(Dj::new(vec![]))).unwrap();
    assert_eq!(
        registry.value("dj", "pick"),
        Some(SettingValue::Text("last".into()))
    );

    registry.set_values(HashMap::from([(
        value_key("dj", "pick"),
        SettingValue::Text("nope".into()),
    )]));
    assert_eq!(
        registry.value("dj", "pick"),
        Some(SettingValue::Text("last".into()))
    );
    registry.set_values(HashMap::from([(
        value_key("dj", "pick"),
        SettingValue::Bool(true),
    )]));
    assert_eq!(
        registry.value("dj", "pick"),
        Some(SettingValue::Text("last".into()))
    );
    registry.set_values(HashMap::from([(
        value_key("dj", "pick"),
        SettingValue::Text("first".into()),
    )]));
    assert_eq!(
        registry.value("dj", "pick"),
        Some(SettingValue::Text("first".into()))
    );
}

#[test]
fn choosing_the_next_video_needs_player_control() {
    let (account, paths) = env();
    let (a, b, c) = (video("a"), video("b"), video("c"));
    let candidates = [b.clone(), c.clone()];

    let mut without = ExtensionRegistry::new();
    without.register(Arc::new(Dj::new(vec![]))).unwrap();
    assert_eq!(without.choose_next(&a, &candidates, &account, &paths), None);
    assert_eq!(
        without.transition(&a, &b, &account, &paths),
        Transition::Cut
    );

    let mut with = ExtensionRegistry::new();
    with.register(Arc::new(Dj::new(vec![Permission::PlayerControl])))
        .unwrap();
    assert_eq!(with.choose_next(&a, &candidates, &account, &paths), Some(1));
    assert_eq!(with.choose_next(&a, &[], &account, &paths), None);
    assert!(matches!(
        with.transition(&a, &b, &account, &paths),
        Transition::Crossfade(_)
    ));

    with.set_disabled(["dj"]);
    assert_eq!(with.choose_next(&a, &candidates, &account, &paths), None);
}

#[tokio::test]
async fn hooks_run_for_enabled_extensions_only() {
    let (account, paths) = env();
    let dj = Dj::new(vec![]);
    let watched = dj.watched.clone();
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(dj)).unwrap();

    registry.watched(&video("a"), &account, &paths).await;
    registry.set_disabled(["dj"]);
    registry.watched(&video("b"), &account, &paths).await;
    assert_eq!(*watched.lock().unwrap(), ["a"]);
}

#[tokio::test]
async fn core_home_uses_the_feed_source_and_the_chips_extension() {
    let (account, paths) = env();
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(Dj::new(vec![]))).unwrap();
    for builtin in builtin::all() {
        registry.register(builtin).unwrap();
    }
    let core = Core::with_extensions(paths, registry);

    // home-chips drops the playlist-only "Mixes".
    let page = core.home(&account, None).await.unwrap();
    let titles: Vec<_> = page.chips.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["Music"]);
    let page = core.home(&account, Some("Music")).await.unwrap();
    assert_eq!(page.videos[0].id.id, "Music");

    // Without home-chips, Home has no chip bar.
    core.extensions().set_disabled(["home-chips"]);
    assert!(core.home(&account, None).await.unwrap().chips.is_empty());

    // Without any main feed source, Home explains how to get one.
    core.extensions().set_disabled(["dj"]);
    let error = core.home(&account, None).await.unwrap_err().to_string();
    assert!(error.starts_with("No YouTube account is set up"));
}

#[test]
fn the_account_is_active_only_when_connected_and_enabled() {
    let core = Core::new(Paths::new("/tmp/kinora-test"));
    let connected = AccountConfig::new("kinora-account --file x");
    assert!(core.account_active(&connected));
    assert!(!core.account_active(&AccountConfig::anonymous()));
    core.extensions().set_disabled([builtin::account::ID]);
    assert!(!core.account_active(&connected));
}

#[test]
fn manifests_load_from_json() {
    let json = r#"{
        "id": "dj", "name": "DJ", "version": "0.1", "description": "Mixes videos",
        "permissions": ["network", "player-control"],
        "settings": [
            { "key": "fade", "title": "Crossfade", "kind": { "type": "toggle", "default": true } }
        ]
    }"#;
    let manifest: Manifest = serde_json::from_str(json).unwrap();
    manifest.validate().unwrap();
    assert!(manifest.has(Permission::PlayerControl));
    assert!(!manifest.has(Permission::Account));
    assert_eq!(
        manifest.setting("fade").unwrap().kind.default_value(),
        SettingValue::Bool(true)
    );
}
