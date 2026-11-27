//! Errors of the local Wiki store.

use std::fmt;

use thiserror::Error;

use crate::wiki::PrepareError;

/// What the store was doing when the database failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Opening the database file.
    Open,
    /// Applying connection settings.
    Configure,
    /// Creating or upgrading the schema.
    Migrate,
    /// Reading stored pages.
    Read,
    /// Writing pages or chunks.
    Write,
    /// Committing an import.
    Commit,
}

impl fmt::Display for Operation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Open => "opening",
            Self::Configure => "configuring",
            Self::Migrate => "migrating",
            Self::Read => "reading",
            Self::Write => "writing",
            Self::Commit => "committing",
        })
    }
}

/// Why a store operation failed.
///
/// Messages carry operations, identifiers and versions only: never page text,
/// titles or file paths.
#[derive(Debug, Error)]
pub enum StoreError {
    /// `SQLite` reported a failure.
    #[error("database failure while {operation} the store")]
    Database {
        /// What was being attempted.
        operation: Operation,
        /// The underlying `SQLite` error.
        #[source]
        source: rusqlite::Error,
    },
    /// The database was written by a newer version of this program.
    #[error("database schema version {found} is newer than the supported version {supported}")]
    UnsupportedSchema {
        /// The version recorded in the file.
        found: i64,
        /// The newest version this build understands.
        supported: i64,
    },
    /// A page's text could not be prepared for storage.
    #[error("page {page} could not be prepared")]
    Prepare {
        /// The page identifier.
        page: u64,
        /// Why preparation failed.
        #[source]
        source: PrepareError,
    },
    /// The store already holds a newer revision of the page.
    #[error(
        "page {page} has stored revision {stored}, which is newer than offered revision {offered}"
    )]
    StaleRevision {
        /// The page identifier.
        page: u64,
        /// The revision in the store.
        stored: u64,
        /// The older revision that was offered.
        offered: u64,
    },
    /// The store holds the same revision of the page with different content.
    #[error("page {page} revision {revision} differs from the stored copy of that revision")]
    ConflictingRevision {
        /// The page identifier.
        page: u64,
        /// The revision identifier.
        revision: u64,
    },
    /// An identifier does not fit the database's integer type.
    #[error("an identifier is too large to store")]
    IdentifierTooLarge,
    /// A stored value is not one this program could have written.
    #[error("the database holds an invalid {what}")]
    Corrupt {
        /// Which kind of value was invalid.
        what: &'static str,
    },
}

impl StoreError {
    pub(super) const fn database(operation: Operation) -> impl FnOnce(rusqlite::Error) -> Self {
        move |source| Self::Database { operation, source }
    }
}
