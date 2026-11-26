//! Errors from Hardcore Grand Exchange queries.

use thiserror::Error;

use super::model::Endpoint;

/// Broad class of transport failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TransportKind {
    /// The request or its connection timed out.
    #[error("timeout")]
    Timeout,
    /// The connection could not be established.
    #[error("connection failure")]
    Connect,
    /// Any other transport failure, such as a reset or a redirect refusal.
    #[error("other transport failure")]
    Other,
}

/// Why a query failed. Missing prices and stale data are not errors; they are
/// represented in the typed results.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MarketDataError {
    /// The request did not complete.
    #[error("request to Hardcore {} endpoint failed: {kind}", .endpoint.path())]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_written_out_and_do_not_depend_on_debug_formatting() {
        let transport = |kind| MarketDataError::Transport {
            endpoint: Endpoint::Latest,
            kind,
        };
        assert_eq!(
            transport(TransportKind::Timeout).to_string(),
            "request to Hardcore latest endpoint failed: timeout"
        );
        assert_eq!(
            transport(TransportKind::Connect).to_string(),
            "request to Hardcore latest endpoint failed: connection failure"
        );
        assert_eq!(
            transport(TransportKind::Other).to_string(),
            "request to Hardcore latest endpoint failed: other transport failure"
        );
    }

    #[test]
    fn a_malformed_message_names_the_field_only_when_there_is_one() {
        let malformed = |field| MarketDataError::Malformed {
            endpoint: Endpoint::Mapping,
            field,
            reason: "required field is missing",
        };
        assert_eq!(
            malformed(Some("id")).to_string(),
            "Hardcore mapping response was malformed: required field is missing (field `id`)"
        );
        assert_eq!(
            malformed(None).to_string(),
            "Hardcore mapping response was malformed: required field is missing"
        );
    }
}
