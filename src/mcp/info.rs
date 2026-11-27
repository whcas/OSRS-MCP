//! The `server_info` tool: static facts about this server.

use std::collections::BTreeMap;

use rmcp::{
    Json,
    model::{ErrorData, ServerCapabilities},
    schemars::JsonSchema,
    serde_json, tool, tool_router,
};
use serde::Serialize;

use super::{OsrsMcpServer, server_capabilities};
use crate::build_info;

/// Structured result of the `server_info` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ServerInfoOutput {
    /// Server package name.
    pub name: String,
    /// Server package version (semantic version).
    pub version: String,
    /// Always true: the server never modifies game state or local data.
    pub read_only: bool,
    /// MCP capability groups this server provides, such as `tools`.
    pub capabilities: Vec<String>,
}

#[tool_router(router = info_router, vis = "pub(super)")]
impl OsrsMcpServer {
    /// Reports this server's name, version, read-only guarantee and capabilities.
    #[tool(
        name = "server_info",
        description = "Report this server's name, version, read-only guarantee and MCP \
                       capabilities. Deterministic; uses no external data.",
        annotations(
            title = "Server information",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn server_info() -> Result<Json<ServerInfoOutput>, ErrorData> {
        server_info_output()
            .map(Json)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))
    }
}

/// Names of the capability groups declared in `capabilities`, sorted.
///
/// Derived from the serialised form, in which an undeclared group is omitted,
/// so a group added by a future rmcp release is reported without a code change.
///
/// # Errors
///
/// Returns an error if the capabilities do not serialise to a JSON object,
/// which would be a bug in the SDK. It is reported rather than turned into an
/// empty list, so `server_info` never under-reports.
fn capability_names(capabilities: &ServerCapabilities) -> Result<Vec<String>, serde_json::Error> {
    let groups: BTreeMap<String, serde_json::Value> =
        serde_json::from_value(serde_json::to_value(capabilities)?)?;
    Ok(groups.into_keys().collect())
}

/// Builds the `server_info` payload from compile-time package metadata.
fn server_info_output() -> Result<ServerInfoOutput, serde_json::Error> {
    let info = build_info();
    Ok(ServerInfoOutput {
        name: info.name.to_owned(),
        version: info.version.to_owned(),
        read_only: true,
        capabilities: capability_names(&server_capabilities())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(capabilities: &ServerCapabilities) -> Vec<String> {
        capability_names(capabilities).unwrap()
    }

    #[test]
    fn no_declared_capabilities_yield_no_names() {
        assert_eq!(names(&ServerCapabilities::default()), Vec::<String>::new());
    }

    #[test]
    fn declared_groups_are_named_and_undeclared_ones_are_not() {
        let tools_only = ServerCapabilities::builder().enable_tools().build();
        assert_eq!(names(&tools_only), ["tools"]);
    }

    #[test]
    fn every_declared_group_is_reported_including_ones_the_old_table_lacked() {
        let everything = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_prompts()
            .enable_completions()
            .enable_experimental()
            .build();
        assert_eq!(
            names(&everything),
            [
                "completions",
                "experimental",
                "prompts",
                "resources",
                "tools"
            ]
        );
    }

    #[test]
    fn the_server_advertises_only_the_tools_capability() {
        assert_eq!(names(&server_capabilities()), ["tools"]);
    }

    #[test]
    fn server_info_describes_this_package_as_read_only() {
        let info = build_info();
        let output = server_info_output().unwrap();
        assert_eq!(
            output,
            ServerInfoOutput {
                name: info.name.to_owned(),
                version: info.version.to_owned(),
                read_only: true,
                capabilities: vec!["tools".to_owned()],
            }
        );
    }
}
