//! Shared fixtures for the MCP tool tests: a fake market source and a helper
//! that connects the real SDK client to a server reading from it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::{RoleClient, RunningService},
};
use serde_json::Value;

use super::OsrsMcpServer;
use super::market_data::{MarketData, MarketFuture};
use crate::domain::{ItemId, UnixSeconds};
use crate::market::{
    Endpoint, ItemInfo, LatestPrices, MarketDataError, Observed, Provenance, WorldType,
};

type Response<T> = Result<Observed<T>, MarketDataError>;

/// Provenance for a response from `endpoint`, retrieved at `retrieved_at`.
pub(super) fn provenance(endpoint: Endpoint, retrieved_at: u64) -> Provenance {
    Provenance {
        world_type: WorldType::Hardcore,
        endpoint,
        source_url: format!("https://example.invalid/{}", endpoint.path()),
        retrieved_at: UnixSeconds(retrieved_at),
    }
}

/// A market source returning canned responses and counting requests. A method a
/// test did not configure fails the test if it is called.
#[derive(Debug, Default)]
pub(super) struct FakeMarket {
    mapping: Option<Response<Vec<ItemInfo>>>,
    latest: Option<Response<BTreeMap<ItemId, LatestPrices>>>,
    requests: AtomicUsize,
}

impl FakeMarket {
    pub(super) fn with_mapping(mapping: Response<Vec<ItemInfo>>) -> Arc<Self> {
        Arc::new(Self {
            mapping: Some(mapping),
            ..Self::default()
        })
    }

    pub(super) fn with_latest(latest: Response<BTreeMap<ItemId, LatestPrices>>) -> Arc<Self> {
        Arc::new(Self {
            latest: Some(latest),
            ..Self::default()
        })
    }

    /// Requests made through any method so far.
    pub(super) fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl MarketData for FakeMarket {
    fn mapping(&self) -> MarketFuture<'_, Vec<ItemInfo>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let result = self.mapping.clone().expect("mapping was not configured");
        Box::pin(async move { result })
    }

    fn latest(&self, _item: ItemId) -> MarketFuture<'_, BTreeMap<ItemId, LatestPrices>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let result = self.latest.clone().expect("latest was not configured");
        Box::pin(async move { result })
    }
}

/// Connects a real SDK client to a server reading from `market`.
pub(super) async fn connect(market: &Arc<FakeMarket>) -> RunningService<RoleClient, ()> {
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let server = OsrsMcpServer::with_market(market.clone());
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    ().serve(client_io).await.expect("client should initialise")
}

/// Calls `tool` with JSON object `arguments`.
pub(super) async fn call_tool(
    client: &RunningService<RoleClient, ()>,
    tool: &'static str,
    arguments: Value,
) -> CallToolResult {
    let arguments = arguments.as_object().expect("object arguments").clone();
    client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
        .expect("tool call should complete")
}

/// The text of a tool error result, asserting that it is one.
pub(super) fn error_text(result: &CallToolResult) -> String {
    assert_eq!(result.is_error, Some(true), "result: {result:?}");
    result
        .content
        .first()
        .and_then(|block| block.as_text())
        .expect("text content")
        .text
        .clone()
}
