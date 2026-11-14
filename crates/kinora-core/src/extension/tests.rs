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

/// A made-up "DJ" (not the built-in one) that plays the last candidate with a
/// crossfade, and a feed with chips.
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
            manifest: manifest("test-dj", permissions, settings),
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
        let id = request.chip.as_deref().unwrap_or("all").to_owned()
            + if request.playlists { "+playlists" } else { "" };
        Box::pin(ready(Ok(HomePage {
            videos: vec![video(&id)],
            chips: vec![chip("Music"), chip("Mixes")],
            ..Default::default()
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
    assert_eq!(panels.len(), 3);
    assert!(!panels[0].enabled && panels[0].builtin);
    assert_eq!(panels[0].values["mark-watched"], SettingValue::Bool(true));
}

#[test]
fn stored_values_that_do_not_fit_are_ignored() {
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(Dj::new(vec![]))).unwrap();
    assert_eq!(
        registry.value("test-dj", "pick"),
        Some(SettingValue::Text("last".into()))
    );

    registry.set_values(HashMap::from([(
        value_key("test-dj", "pick"),
        SettingValue::Text("nope".into()),
    )]));
    assert_eq!(
        registry.value("test-dj", "pick"),
        Some(SettingValue::Text("last".into()))
    );
    registry.set_values(HashMap::from([(
        value_key("test-dj", "pick"),
        SettingValue::Bool(true),
    )]));
    assert_eq!(
        registry.value("test-dj", "pick"),
        Some(SettingValue::Text("last".into()))
    );
    registry.set_values(HashMap::from([(
        value_key("test-dj", "pick"),
        SettingValue::Text("first".into()),
    )]));
    assert_eq!(
        registry.value("test-dj", "pick"),
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

    with.set_disabled(["test-dj"]);
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
    registry.set_disabled(["test-dj"]);
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

    // home-chips keeps the playlist-only "Mixes" unless told to hide it.
    let page = core.home(&account, None).await.unwrap();
    let titles: Vec<_> = page.chips.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["Music", "Mixes"]);
    let page = core.home(&account, Some("Music")).await.unwrap();
    assert_eq!(page.videos[0].id.id, "Music");
    // Asking for playlists tells the source so.
    let page = core
        .home_with_playlists(&account, Some("Mixes"))
        .await
        .unwrap();
    assert_eq!(page.videos[0].id.id, "Mixes+playlists");
    let page = core
        .home_more_with_playlists(&account, "tok")
        .await
        .unwrap();
    assert_eq!(page.videos[0].id.id, "all+playlists");

    // Without home-chips, Home has no chip bar.
    core.extensions().set_disabled(["home-chips"]);
    assert!(core.home(&account, None).await.unwrap().chips.is_empty());

    // Without any main feed source, Home explains how to get one.
    core.extensions().set_disabled(["test-dj"]);
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

#[tokio::test]
async fn the_dj_follows_its_switch_and_settings() {
    use crate::dj::{DjSeed, Track};
    use builtin::dj::{CROSSFADE_SECONDS, MY_MIX, START_FROM};
    use std::time::Duration;

    let core = Core::new(Paths::new("/tmp/kinora-test"));
    let anonymous = AccountConfig::anonymous();
    let connected = AccountConfig::new("kinora-account --file x");
    let open = VideoId {
        id: "dQw4w9WgXcQ".into(),
        platform: "youtube".into(),
    };
    let radio = DjSeed::Video(open.id.clone());

    // Start from the open video; "my mix" without one.
    assert_eq!(core.dj_seed(&connected, Some(&open)), radio);
    assert_eq!(core.dj_seed(&anonymous, None), DjSeed::MyMix);
    let peertube = VideoId {
        id: "x".into(),
        platform: "peertube".into(),
    };
    assert_eq!(core.dj_seed(&anonymous, Some(&peertube)), DjSeed::MyMix);

    // "My mix" needs the account, else the open video it is.
    core.extensions().set_values(HashMap::from([
        (
            value_key("dj", START_FROM),
            SettingValue::Text(MY_MIX.into()),
        ),
        (
            value_key("dj", CROSSFADE_SECONDS),
            SettingValue::Text("3".into()),
        ),
    ]));
    assert_eq!(core.dj_seed(&connected, Some(&open)), DjSeed::MyMix);
    assert_eq!(core.dj_seed(&anonymous, Some(&open)), radio);
    core.extensions().set_disabled([builtin::account::ID]);
    assert_eq!(core.dj_seed(&connected, Some(&open)), radio);

    // The crossfade setting shapes the plan.
    let next = Track::from_video(&video("b"));
    assert!(core.dj_transition(None, Duration::ZERO, &next).is_cut());
    let mut long = next.clone();
    long.duration = Some(Duration::from_secs(200));
    let plan = core.dj_transition(Some(Duration::from_secs(200)), Duration::ZERO, &long);
    assert_eq!(plan.fade, Duration::from_secs(3));
    assert_eq!(plan.start_at, Some(Duration::from_secs(197)));

    // Frontends spawn these on their runtime.
    fn is_send<T: Send>(_: &T) {}
    let mut session = crate::dj::DjSession::start(
        radio.clone(),
        Default::default(),
        crate::dj::RadioPage {
            tracks: vec![],
            continuation: None,
            source: crate::dj::RadioSource::Music,
            personal: false,
        },
    );
    is_send(&core.dj_start(&anonymous, radio.clone()));
    is_send(&core.dj_more(&anonymous, &crate::dj::RadioRequest::Start(radio.clone())));
    is_send(&core.dj_refill(&anonymous, &mut session));

    // Off, the DJ does nothing (and fetches nothing).
    core.extensions().set_disabled(["dj"]);
    let error = core.dj_start(&anonymous, radio).await.unwrap_err();
    assert!(error.to_string().contains("switched off"), "{error}");
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
