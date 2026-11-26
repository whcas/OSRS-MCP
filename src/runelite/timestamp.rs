//! The `exported_at` timestamp.
//!
//! The plugin writes `OffsetDateTime.now().toString()`: ISO-8601 with an
//! offset, seconds omitted when zero and a fraction printed in groups of three
//! digits. Parsing is delegated to `jiff`, which accepts all of those forms.

use std::str::FromStr;

use jiff::Timestamp;

/// A parsed `exported_at` value with its original text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedAt {
    text: String,
    timestamp: Timestamp,
}

impl ExportedAt {
    /// Parses the plugin's timestamp text, or `None` if `jiff` rejects it.
    ///
    /// Every form the plugin writes is accepted, including a missing seconds
    /// field (`2026-01-01T12:00Z`), one to nine fraction digits and an offset
    /// such as `+01:00` or `+01:00:30`. Nanosecond precision is kept.
    ///
    /// Parsing is deliberately lenient where `jiff` is. It also accepts, for
    /// example, a space or lowercase `t` as the separator, a lowercase `z`,
    /// offsets beyond 18 hours and basic format (`20260101T120000Z`). The
    /// plugin never writes these, and a malformed value still fails.
    ///
    /// **Leap seconds are normalised.** A seconds field of `60` is read as
    /// `59` (`jiff` does not represent leap seconds), so `12:00:60Z` and
    /// `12:00:59Z` are the same instant. Java's `OffsetDateTime` cannot
    /// produce a leap second, so this only affects hand-edited input.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let timestamp = Timestamp::from_str(text).ok()?;
        Some(Self {
            text: text.to_owned(),
            timestamp,
        })
    }

    /// The original text, exactly as exported.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The instant, with nanosecond precision.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }
}
