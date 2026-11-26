//! Errors from Hardcore Grand Exchange queries.

use std::fmt;

use super::model::Endpoint;

/// Broad class of transport failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// The request or its connection timed out.
    Timeout,
    /// The connection could not be established.
    Connect,
    /// Any other transport failure, such as a reset or a redirect refusal.
    Other,
}

/// Why a query failed. Missing prices and stale data are not errors; they are
/// represented in the typed results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketDataError {
    /// The request did not complete.
    Transport {
        /// The endpoint requested.
        endpoint: Endpoint,
        /// What kind of failure it was.
        kind: TransportKind,
    },
    /// The server answered with a non-success status, such as 429 or 403.
    Http {
        /// The endpoint requested.
        endpoint: Endpoint,
        /// The HTTP status code.
        status: u16,
    },
    /// The body was not the documented shape.
    ///
    /// This carries only a fixed description and, where one applies, the name
    /// of the offending field. It never includes response text, so an
    /// oversized or hostile body cannot produce an oversized diagnostic.
    Malformed {
        /// The endpoint requested.
        endpoint: Endpoint,
        /// The documented field that was missing or invalid, if one applies.
        field: Option<&'static str>,
        /// What was wrong, as fixed text.
        reason: &'static str,
    },
    /// The body exceeded the configured size limit and was discarded.
    ResponseTooLarge {
        /// The endpoint requested.
        endpoint: Endpoint,
        /// The limit in bytes.
        limit_bytes: usize,
    },
    /// The system clock reads before the Unix epoch, so a retrieval time
    /// cannot be recorded. Reported rather than stored as 0, which would look
    /// like very stale data.
    ClockBeforeEpoch,
}

impl fmt::Display for MarketDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { endpoint, kind } => write!(
                f,
                "request to Hardcore {} endpoint failed: {kind:?}",
                endpoint.path()
            ),
            Self::Http { endpoint, status } => write!(
                f,
                "Hardcore {} endpoint returned HTTP {status}",
                endpoint.path()
            ),
            Self::Malformed {
                endpoint,
                field: Some(field),
                reason,
            } => write!(
                f,
                "Hardcore {} response was malformed: {reason} (field `{field}`)",
                endpoint.path()
            ),
            Self::Malformed {
                endpoint,
                field: None,
                reason,
            } => write!(
                f,
                "Hardcore {} response was malformed: {reason}",
                endpoint.path()
            ),
            Self::ResponseTooLarge {
                endpoint,
                limit_bytes,
            } => write!(
                f,
                "Hardcore {} response exceeded {limit_bytes} bytes",
                endpoint.path()
            ),
            Self::ClockBeforeEpoch => f.write_str("system clock is before the Unix epoch"),
        }
    }
}

impl std::error::Error for MarketDataError {}
