//! Errors from Hardcore Grand Exchange queries.

use thiserror::Error;

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
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MarketDataError {
    /// The request did not complete.
    #[error("request to Hardcore {} endpoint failed: {kind:?}", .endpoint.path())]
    Transport {
        /// The endpoint requested.
        endpoint: Endpoint,
        /// What kind of failure it was.
        kind: TransportKind,
    },
    /// The server answered with a non-success status, such as 429 or 403.
    #[error("Hardcore {} endpoint returned HTTP {status}", .endpoint.path())]
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
    #[error("Hardcore {} response was malformed: {reason}{}", .endpoint.path(), field_note(*.field))]
    Malformed {
        /// The endpoint requested.
        endpoint: Endpoint,
        /// The documented field that was missing or invalid, if one applies.
        field: Option<&'static str>,
        /// What was wrong, as fixed text.
        reason: &'static str,
    },
    /// The body exceeded the configured size limit and was discarded.
    #[error("Hardcore {} response exceeded {limit_bytes} bytes", .endpoint.path())]
    ResponseTooLarge {
        /// The endpoint requested.
        endpoint: Endpoint,
        /// The limit in bytes.
        limit_bytes: usize,
    },
    /// The system clock reads before the Unix epoch, so a retrieval time
    /// cannot be recorded. Reported rather than stored as 0, which would look
    /// like very stale data.
    #[error("system clock is before the Unix epoch")]
    ClockBeforeEpoch,
}

/// The text appended to a malformed-response message for the offending field.
fn field_note(field: Option<&str>) -> String {
    field.map_or_else(String::new, |field| format!(" (field `{field}`)"))
}
