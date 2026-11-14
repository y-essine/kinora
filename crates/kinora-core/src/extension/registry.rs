//! The registry: every extension, which ones are on, and their settings.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use pcore::Video;

use super::{
    ActionOutcome, Context, Extension, FeedFilter, FeedPlacement, FeedSource, MenuItem,
    MenuLocation, MenuTarget, Permission, PlayerHook, SettingValue, SettingsPanel, value_key,
};
use crate::player::Transition;
use crate::{AccountConfig, Error, Paths};

/// One registered extension and its contributions (asked for once).
struct Entry {
    extension: Arc<dyn Extension>,
    feed_sources: Vec<Arc<dyn FeedSource>>,
    feed_filters: Vec<Arc<dyn FeedFilter>>,
    player_hooks: Vec<Arc<dyn PlayerHook>>,
    menu_items: Vec<MenuItem>,
}

impl Entry {
    fn id(&self) -> &str {
        &self.extension.manifest().id
    }
}

#[derive(Default)]
struct State {
    disabled: HashSet<String>,
    /// Stored values by [`value_key`]; may hold stale or unknown keys.
    values: HashMap<String, SettingValue>,
}

/// Holds the extensions in registration order. Every extension is enabled
/// unless its id is in the disabled list the frontend passes in (the GTK app
/// keeps it in the `disabled-extensions` GSettings key).
///
/// Enabling and settings can change at any time from any thread; the
/// contributions themselves are fixed once registered.
pub struct ExtensionRegistry {
    entries: Vec<Entry>,
    state: RwLock<State>,
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ExtensionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionRegistry")
            .field(
                "extensions",
                &self.entries.iter().map(Entry::id).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl ExtensionRegistry {
    /// No extensions at all.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            state: RwLock::default(),
        }
    }

    /// The extensions that ship with Kinora (see [`super::builtin`]).
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        for extension in super::builtin::all() {
            registry
                .register(extension)
                .expect("built-in extensions to be valid");
        }
        registry
    }

    /// Add an extension. Fails for an invalid manifest or a taken id.
    pub fn register(&mut self, extension: Arc<dyn Extension>) -> Result<(), Error> {
        let manifest = extension.manifest();
        manifest.validate()?;
        if self.entry(&manifest.id).is_some() {
            return Err(format!("extension {:?} is registered twice", manifest.id).into());
        }
        self.entries.push(Entry {
            feed_sources: extension.feed_sources(),
            feed_filters: extension.feed_filters(),
            player_hooks: extension.player_hooks(),
            menu_items: extension.menu_items(),
            extension,
        });
        Ok(())
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id() == id)
    }

    fn state(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn state_mut(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Every registered extension, enabled or not.
    pub fn extensions(&self) -> impl Iterator<Item = &Arc<dyn Extension>> {
        self.entries.iter().map(|e| &e.extension)
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn Extension>> {
        self.entry(id).map(|e| &e.extension)
    }

    /// Registered and not disabled.
    pub fn is_enabled(&self, id: &str) -> bool {
        self.entry(id).is_some() && !self.state().disabled.contains(id)
    }

    /// Replace the disabled list. Unknown ids are kept (an extension that is
    /// not installed right now stays disabled when it comes back).
    pub fn set_disabled<I, S>(&self, ids: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.state_mut().disabled = ids.into_iter().map(Into::into).collect();
    }

    /// Switch one extension on or off; returns the new disabled list (sorted),
    /// for the frontend to store.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Vec<String> {
        let mut state = self.state_mut();
        if enabled {
            state.disabled.remove(id);
        } else {
            state.disabled.insert(id.to_owned());
        }
        let mut list: Vec<String> = state.disabled.iter().cloned().collect();
        list.sort();
        list
    }

    /// Replace the stored setting values, keyed by [`value_key`]. Values that
    /// do not fit their setting are ignored (the default is used instead).
    pub fn set_values(&self, values: HashMap<String, SettingValue>) {
        self.state_mut().values = values;
    }

    /// The current values of an extension's settings (defaults filled in).
    fn values_of(&self, entry: &Entry) -> HashMap<String, SettingValue> {
        let state = self.state();
        let manifest = entry.extension.manifest();
        manifest
            .settings
            .iter()
            .map(|setting| {
                let value = state
                    .values
                    .get(&value_key(&manifest.id, &setting.key))
                    .filter(|v| setting.kind.accepts(v))
                    .cloned()
                    .unwrap_or_else(|| setting.kind.default_value());
                (setting.key.clone(), value)
            })
            .collect()
    }

    /// The current value of one setting.
    pub fn value(&self, extension: &str, key: &str) -> Option<SettingValue> {
        self.entry(extension)
            .and_then(|entry| self.values_of(entry).remove(key))
    }

    fn context_for(&self, entry: &Entry, account: &AccountConfig, paths: &Paths) -> Context {
        let manifest = entry.extension.manifest();
        Context {
            extension: manifest.id.clone(),
            permissions: manifest.permissions.clone(),
            // Only extensions with the permission ever see the account.
            account: if manifest.has(Permission::Account) {
                account.clone()
            } else {
                AccountConfig::anonymous()
            },
            paths: paths.clone(),
            values: self.values_of(entry),
        }
    }

    /// The context an extension's code runs with.
    pub fn context(
        &self,
        extension: &str,
        account: &AccountConfig,
        paths: &Paths,
    ) -> Option<Context> {
        self.entry(extension)
            .map(|entry| self.context_for(entry, account, paths))
    }

    fn enabled_entries(&self) -> impl Iterator<Item = &Entry> {
        let disabled = self.state().disabled.clone();
        self.entries
            .iter()
            .filter(move |e| !disabled.contains(e.id()))
    }

    /// The first enabled, available feed source for `placement`, with the
    /// context to load it with.
    pub fn feed_source(
        &self,
        placement: FeedPlacement,
        account: &AccountConfig,
        paths: &Paths,
    ) -> Option<(Arc<dyn FeedSource>, Context)> {
        self.feed_sources(placement, account, paths)
            .into_iter()
            .next()
    }

    /// Every enabled, available feed source for `placement`, in order.
    pub fn feed_sources(
        &self,
        placement: FeedPlacement,
        account: &AccountConfig,
        paths: &Paths,
    ) -> Vec<(Arc<dyn FeedSource>, Context)> {
        let mut sources = Vec::new();
        for entry in self.enabled_entries() {
            let ctx = self.context_for(entry, account, paths);
            for source in &entry.feed_sources {
                if source.placement() == placement && source.available(&ctx) {
                    sources.push((source.clone(), ctx.clone()));
                }
            }
        }
        sources
    }

    /// Run the enabled extensions' feed filters over a Home page.
    pub fn filter_feed(&self, page: &mut crate::HomePage, account: &AccountConfig, paths: &Paths) {
        for entry in self.enabled_entries() {
            if entry.feed_filters.is_empty() {
                continue;
            }
            let ctx = self.context_for(entry, account, paths);
            for filter in &entry.feed_filters {
                filter.filter(&ctx, page);
            }
        }
    }

    /// Tell the enabled extensions' hooks that a video was watched. Errors are
    /// logged; one failing hook does not stop the others.
    pub async fn watched(&self, video: &Video, account: &AccountConfig, paths: &Paths) {
        let calls: Vec<_> = self
            .enabled_entries()
            .flat_map(|entry| {
                let ctx = self.context_for(entry, account, paths);
                entry.player_hooks.iter().map(move |hook| {
                    (
                        ctx.extension.clone(),
                        hook.on_watched(ctx.clone(), video.clone()),
                    )
                })
            })
            .collect();
        for (extension, call) in calls {
            if let Err(e) = call.await {
                log::warn!("{extension}: {e}");
            }
        }
    }

    /// Tell the enabled extensions' hooks that a video played to its end.
    pub async fn ended(&self, video: &Video, account: &AccountConfig, paths: &Paths) {
        let calls: Vec<_> = self
            .enabled_entries()
            .flat_map(|entry| {
                let ctx = self.context_for(entry, account, paths);
                entry.player_hooks.iter().map(move |hook| {
                    (
                        ctx.extension.clone(),
                        hook.on_ended(ctx.clone(), video.clone()),
                    )
                })
            })
            .collect();
        for (extension, call) in calls {
            if let Err(e) = call.await {
                log::warn!("{extension}: {e}");
            }
        }
    }

    /// Which of `candidates` plays after `current`: the first answer of an
    /// enabled hook with [`Permission::PlayerControl`], else `None` (the
    /// frontend's default: the first candidate).
    pub fn choose_next(
        &self,
        current: &Video,
        candidates: &[Video],
        account: &AccountConfig,
        paths: &Paths,
    ) -> Option<usize> {
        self.player_control_hooks(account, paths)
            .find_map(|(hook, ctx)| hook.choose_next(&ctx, current, candidates))
            .filter(|&i| i < candidates.len())
    }

    /// How to go from `current` to `next`; [`Transition::Cut`] unless a hook
    /// with [`Permission::PlayerControl`] says otherwise.
    pub fn transition(
        &self,
        current: &Video,
        next: &Video,
        account: &AccountConfig,
        paths: &Paths,
    ) -> Transition {
        self.player_control_hooks(account, paths)
            .find_map(|(hook, ctx)| hook.transition(&ctx, current, next))
            .unwrap_or_default()
    }

    fn player_control_hooks<'a>(
        &'a self,
        account: &'a AccountConfig,
        paths: &'a Paths,
    ) -> impl Iterator<Item = (&'a Arc<dyn PlayerHook>, Context)> + 'a {
        self.enabled_entries()
            .filter(|e| e.extension.manifest().has(Permission::PlayerControl))
            .flat_map(move |entry| {
                let ctx = self.context_for(entry, account, paths);
                entry
                    .player_hooks
                    .iter()
                    .map(move |hook| (hook, ctx.clone()))
            })
    }

    /// The enabled extensions' menu items for one menu, with their extension's id.
    pub fn menu_items(&self, location: MenuLocation) -> Vec<(String, MenuItem)> {
        self.enabled_entries()
            .flat_map(|entry| {
                entry
                    .menu_items
                    .iter()
                    .filter(|item| item.location == location)
                    .map(|item| (entry.id().to_owned(), item.clone()))
            })
            .collect()
    }

    /// Run a menu item of an enabled extension.
    pub async fn activate(
        &self,
        extension: &str,
        item: &str,
        target: MenuTarget,
        account: &AccountConfig,
        paths: &Paths,
    ) -> Result<ActionOutcome, Error> {
        let entry = self
            .entry(extension)
            .filter(|_| self.is_enabled(extension))
            .ok_or_else(|| format!("The {extension} extension is not enabled"))?;
        let ctx = self.context_for(entry, account, paths);
        entry.extension.activate(ctx, item, target).await
    }

    /// One panel per registered extension, enabled or not, for the frontend's
    /// preferences.
    pub fn settings_panels(&self) -> Vec<SettingsPanel> {
        self.entries
            .iter()
            .map(|entry| {
                let manifest = entry.extension.manifest();
                SettingsPanel {
                    extension: manifest.id.clone(),
                    title: manifest.name.clone(),
                    description: manifest.description.clone(),
                    builtin: manifest.builtin,
                    enabled: self.is_enabled(&manifest.id),
                    permissions: manifest.permissions.clone(),
                    settings: manifest.settings.clone(),
                    values: self.values_of(entry),
                }
            })
            .collect()
    }
}
