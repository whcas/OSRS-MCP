//! The boundary between the market tools and the network.

use std::future::Future;
use std::pin::Pin;

use crate::market::{HardcoreClient, ItemInfo, MarketDataError, Observed};

/// A boxed future, so the trait can be used as `dyn` without an extra crate.
pub(super) type MarketFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<Observed<T>, MarketDataError>> + Send + 'a>>;

/// The Hardcore market data the MCP tools read.
///
/// This is deliberately narrow: one method per upstream request a tool makes,
/// each performing exactly one request. [`HardcoreClient`] is the production
/// implementation; tests supply their own fakes.
pub(super) trait MarketData: std::fmt::Debug + Send + Sync {
    /// Item metadata for every Hardcore item. IDs are unique, and each name
    /// is at most 256 UTF-8 bytes.
    fn mapping(&self) -> MarketFuture<'_, Vec<ItemInfo>>;
}

impl MarketData for HardcoreClient {
    fn mapping(&self) -> MarketFuture<'_, Vec<ItemInfo>> {
        Box::pin(Self::mapping(self))
    }
}
