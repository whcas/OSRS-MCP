//! The `source` block shared by market tool outputs.

use rmcp::schemars::JsonSchema;
use serde::Serialize;

use crate::market::{Provenance, WorldType};

/// Where and when the data came from.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(super) struct SourceOutput {
    /// Always `hardcore`: results never come from standard-world data.
    world_type: &'static str,
    /// The upstream endpoint that supplied the data.
    endpoint: &'static str,
    /// The exact URL requested.
    source_url: String,
    /// When this server finished retrieving the response, in whole seconds since
    /// the Unix epoch (UTC). It is not the time of any trade.
    retrieved_at_unix_seconds: u64,
}

impl From<&Provenance> for SourceOutput {
    fn from(provenance: &Provenance) -> Self {
        Self {
            world_type: match provenance.world_type {
                WorldType::Hardcore => "hardcore",
            },
            endpoint: provenance.endpoint.path(),
            source_url: provenance.source_url.clone(),
            retrieved_at_unix_seconds: provenance.retrieved_at.0,
        }
    }
}
