//! Offline Wiki knowledge: importing manually exported `MediaWiki` pages.
//!
//! Everything here is local and offline. Nothing fetches Wiki pages, and all
//! content is untrusted data, never instructions (see ADR-0002 for the
//! acquisition decision and the licensing questions that remain open).
//!
//! # Layout
//!
//! - `export`: the streaming importer for `Special:Export` XML ([`ExportReader`]).
//! - `model`: plain types for a page and its provenance ([`ImportedPage`],
//!   [`SourceId`]).
//! - `sections`: section paths and bounded chunks for retrieval ([`prepare`]).
//! - `timestamp`: the export's fixed-shape revision timestamp.
//!
//! Wiki types stay independent of MCP, storage and the CLI.

mod export;
mod model;
mod sections;
mod timestamp;

pub use export::{
    ExportError, ExportReader, ExportSummary, ImportIssue, ImportLimits, LimitKind, PageField,
    PageIssue, PageIssueKind, XmlFault,
};
pub use model::{ExportedHash, ImportedPage, PageId, PageTitle, RevisionId, SourceField, SourceId};
pub use sections::{
    Chunk, ChunkKind, InvalidLimits, Loss, PrepareError, PrepareLimits, PreparedPage, Section,
    SectionId, SectionPath, prepare,
};
pub use timestamp::{InvalidTimestamp, RevisionTimestamp};
