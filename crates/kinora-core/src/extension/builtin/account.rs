//! The YouTube account as an extension: it supplies Home, personalises Up
//! next, marks played videos watched on YouTube, and imports the account's
//! subscriptions. Off (or no account connected): fully anonymous.
//!
//! Connecting an account (picking a browser profile) stays the frontend's
//! job; it ends up in [`AccountConfig::cookies_command`](crate::AccountConfig).

use std::sync::Arc;

use futures::future::BoxFuture;
use pcore::Video;

use crate::extension::{
    ActionOutcome, Context, Extension, FeedPlacement, FeedRequest, FeedSource, Manifest, MenuItem,
    MenuLocation, MenuTarget, Permission, PlayerHook, Setting,
};
use crate::{Error, HomePage, account, home, youtube};

pub const ID: &str = "account";
/// Setting: use the account for Up next (otherwise it is anonymous).
pub const PERSONAL_UP_NEXT: &str = "personal-up-next";
/// Setting: mark played videos watched on YouTube.
pub const MARK_WATCHED: &str = "mark-watched";
/// Menu item: subscribe to the account's YouTube subscriptions.
pub const IMPORT_SUBSCRIPTIONS: &str = "import-subscriptions";

pub struct AccountExtension {
    manifest: Manifest,
}

impl Default for AccountExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountExtension {
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                id: ID.to_owned(),
                name: "YouTube Account".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                description: "Your YouTube Home, personal Up next and watch history. \
                    Playback stays anonymous."
                    .to_owned(),
                permissions: vec![Permission::Network, Permission::Account],
                settings: vec![
                    Setting::toggle(
                        PERSONAL_UP_NEXT,
                        "Personal Up Next",
                        Some("Recommendations under a video come from the account"),
                        true,
                    ),
                    Setting::toggle(
                        MARK_WATCHED,
                        "Mark Videos Watched on YouTube",
                        Some("So Home and Up next learn from what you watch here"),
                        true,
                    ),
                ],
                builtin: true,
            },
        }
    }
}

impl Extension for AccountExtension {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn feed_sources(&self) -> Vec<Arc<dyn FeedSource>> {
        vec![Arc::new(YouTubeHome)]
    }

    fn player_hooks(&self) -> Vec<Arc<dyn PlayerHook>> {
        vec![Arc::new(MarkWatched)]
    }

    fn menu_items(&self) -> Vec<MenuItem> {
        vec![MenuItem {
            id: IMPORT_SUBSCRIPTIONS.to_owned(),
            label: "Import Subscriptions from Account".to_owned(),
            location: MenuLocation::Home,
        }]
    }

    fn activate(
        &self,
        ctx: Context,
        item: &str,
        _target: MenuTarget,
    ) -> BoxFuture<'static, Result<ActionOutcome, Error>> {
        let item = item.to_owned();
        Box::pin(async move {
            if item != IMPORT_SUBSCRIPTIONS {
                return Err(format!("account: no menu item {item:?}").into());
            }
            ctx.require(Permission::Network)?;
            let cookies = ctx.account()?.cookies().await?.ok_or(
                "No YouTube account is set up. Set the account cookies command in the preferences.",
            )?;
            let channels = account::subscriptions(ctx.paths().rustypipe_dir(), cookies).await?;
            Ok(ActionOutcome::Subscribe(channels))
        })
    }
}

/// The account's YouTube Home page, with its chips.
struct YouTubeHome;

impl FeedSource for YouTubeHome {
    fn id(&self) -> &str {
        "youtube-home"
    }

    fn title(&self) -> &str {
        "Recommended"
    }

    fn placement(&self) -> FeedPlacement {
        FeedPlacement::Main
    }

    fn available(&self, ctx: &Context) -> bool {
        ctx.account().is_ok_and(|a| a.is_connected())
    }

    fn load(
        &self,
        ctx: Context,
        request: FeedRequest,
    ) -> BoxFuture<'static, Result<HomePage, Error>> {
        Box::pin(async move {
            ctx.require(Permission::Network)?;
            let cookies = ctx.account()?.cookies().await?;
            if let Some(token) = request.continuation {
                let cookies = cookies.ok_or("No YouTube account is set up.")?;
                return home::continuation(&cookies, &token).await;
            }
            let cookies = cookies.ok_or(
                "No YouTube account is set up. Set the account cookies command in the preferences.",
            )?;
            home::load(&cookies, request.chip.as_deref()).await
        })
    }
}

/// Tells YouTube a video was watched, so the account's recommendations learn.
struct MarkWatched;

impl PlayerHook for MarkWatched {
    fn on_watched(&self, ctx: Context, video: Video) -> BoxFuture<'static, Result<(), Error>> {
        Box::pin(async move {
            if !ctx.flag(MARK_WATCHED) || !youtube::is_youtube(&video.id) {
                return Ok(());
            }
            let id = video.id.id;
            let result = async {
                ctx.require(Permission::Network)?;
                if let Some(cookies) = ctx.account()?.cookies().await? {
                    account::mark_watched(&cookies, &id).await?;
                    log::info!("Marked {id} watched on the YouTube account");
                }
                Ok::<_, Error>(())
            }
            .await;
            result.map_err(|e| {
                format!("Could not mark {id} watched on the YouTube account: {e}").into()
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::ExtensionRegistry;
    use crate::{AccountConfig, Paths};

    #[test]
    fn home_needs_a_connected_account_and_the_extension() {
        let registry = ExtensionRegistry::with_builtins();
        let paths = Paths::new("/tmp/kinora-test");
        let connected = AccountConfig::new("kinora-account --file x");

        let main = |account: &AccountConfig| {
            registry
                .feed_source(FeedPlacement::Main, account, &paths)
                .map(|(source, _)| source.id().to_owned())
        };
        assert_eq!(main(&connected).as_deref(), Some("youtube-home"));
        assert_eq!(main(&AccountConfig::anonymous()), None);

        registry.set_disabled([ID]);
        assert_eq!(main(&connected), None);
    }

    #[test]
    fn offers_its_menu_item_on_home() {
        let registry = ExtensionRegistry::with_builtins();
        let items = registry.menu_items(MenuLocation::Home);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, ID);
        assert_eq!(items[0].1.id, IMPORT_SUBSCRIPTIONS);
        assert!(registry.menu_items(MenuLocation::Video).is_empty());
    }
}
