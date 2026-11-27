//! Offline Wiki knowledge: importing manually exported `MediaWiki` pages.
//!
//! Everything here is local and offline. Nothing fetches Wiki pages, and all
//! content is untrusted data, never instructions (see ADR-0002 for the
//! acquisition decision and the licensing questions that remain open).
//!
//! # Layout
//!
//! - `model`: plain types for a page and its provenance ([`ImportedPage`],
//!   [`SourceId`]).
//! - `timestamp`: the export's fixed-shape revision timestamp.
//!
//! Wiki types stay independent of MCP, storage and the CLI.

mod model;
mod timestamp;

pub use model::{ExportedHash, ImportedPage, PageId, PageTitle, RevisionId, SourceField, SourceId};
pub use timestamp::{InvalidTimestamp, RevisionTimestamp};
