//! Feed contributions: what Home shows.

use futures::future::BoxFuture;

use super::Context;
use crate::{Error, HomePage};

/// Where a feed source's videos go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeedPlacement {
    /// Home's main grid, with its filter chips. Home shows the first enabled
    /// main source that is available.
    Main,
    /// A row (shelf) on Home, e.g. "Continue watching". Chips are ignored.
    /// No frontend shows rows yet.
    Row,
}

/// Which page a feed source should load.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedRequest {
    /// The token of the filter chip the user picked; `None` for "All".
    pub chip: Option<String>,
    /// The previous page's [`HomePage::next`]: load the page after it.
    pub continuation: Option<String>,
}

/// Something that supplies Home with videos (and maybe chips).
pub trait FeedSource: Send + Sync {
    /// Unique within the extension, e.g. `youtube-home`.
    fn id(&self) -> &str;
    /// A heading for the feed ("Recommended", "Continue watching").
    fn title(&self) -> &str;
    fn placement(&self) -> FeedPlacement;
    /// Whether it can load anything right now (the account's Home needs an
    /// account). Unavailable sources are skipped.
    fn available(&self, _ctx: &Context) -> bool {
        true
    }
    /// Load a page. Videos may lack details (upload date, channel avatar);
    /// the frontend fills them in with the regular video lookup.
    fn load(
        &self,
        ctx: Context,
        request: FeedRequest,
    ) -> BoxFuture<'static, Result<HomePage, Error>>;
}

/// Adjusts every Home page after it is loaded: drops chips or videos,
/// reorders them. Filters run in registration order.
pub trait FeedFilter: Send + Sync {
    fn filter(&self, ctx: &Context, page: &mut HomePage);
}
