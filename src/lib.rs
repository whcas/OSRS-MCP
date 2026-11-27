//! Core library for the OSRS Hardcore worlds advisor.
//!
//! Domain logic lives here, independent of any transport. The binary in
//! `src/main.rs` is a thin adapter over this crate.

pub mod domain;
pub mod market;
pub mod mcp;
pub mod profit;
pub mod wiki;

/// Static build metadata for this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildInfo {
    /// Package name, as declared in `Cargo.toml`.
    pub name: &'static str,
    /// Package version, as declared in `Cargo.toml`.
    pub version: &'static str,
}

/// Returns build metadata taken from Cargo at compile time.
#[must_use]
pub const fn build_info() -> BuildInfo {
    BuildInfo {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_reports_cargo_package_metadata() {
        let info = build_info();
        assert_eq!(info.name, "osrs-mcp");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }
}
