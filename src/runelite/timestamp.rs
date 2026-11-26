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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_in_every_form_java_can_print_are_accepted() {
        let cases = [
            ("2026-01-01T12:00Z", 1_767_268_800, 0),
            ("2026-01-01T12:00:30Z", 1_767_268_830, 0),
            ("2026-01-01T12:00:00.5Z", 1_767_268_800, 500_000_000),
            ("2026-01-01T12:00:00.123Z", 1_767_268_800, 123_000_000),
            ("2026-01-01T12:00:00.123456789Z", 1_767_268_800, 123_456_789),
            ("2026-01-01T13:00:00+01:00", 1_767_268_800, 0),
            ("2026-01-01T07:00:00-05:00", 1_767_268_800, 0),
            ("2024-02-29T00:00Z", 1_709_164_800, 0),
            ("1970-01-01T00:00Z", 0, 0),
        ];
        for (text, seconds, nanos) in cases {
            let at = ExportedAt::parse(text).unwrap_or_else(|| panic!("{text} should parse"));
            assert_eq!(at.timestamp().as_second(), seconds, "{text}");
            assert_eq!(at.timestamp().subsec_nanosecond(), nanos, "{text}");
            assert_eq!(at.text(), text);
        }
    }

    #[test]
    fn malformed_timestamps_are_rejected() {
        let cases = [
            "",
            "2026-01-01",
            "2026-01-01T12:00",
            "2026-13-01T12:00Z",
            "2026-02-29T12:00Z",
            "2026-01-32T12:00Z",
            "2026-01-01T24:00Z",
            "2026-01-01T12:60Z",
            "2026-01-01T12:00:00.Z",
            "2026-01-01T12:00:00.1234567890Z",
            "2026-01-01T12:00Zjunk",
            "2026-01-01T12:00:00 Z",
            "2026-01-01T12:00:00[UTC]",
        ];
        for text in cases {
            assert!(
                ExportedAt::parse(text).is_none(),
                "{text:?} should be rejected"
            );
        }
    }

    #[test]
    fn a_leap_second_is_normalised_to_the_previous_second() {
        let leap = ExportedAt::parse("2026-01-01T12:00:60Z").expect("accepted");
        let normal = ExportedAt::parse("2026-01-01T12:00:59Z").expect("valid");
        assert_eq!(leap.timestamp(), normal.timestamp());
        assert_eq!(leap.timestamp().as_second(), 1_767_268_859);
        // The original text is still the text that was exported.
        assert_eq!(leap.text(), "2026-01-01T12:00:60Z");
    }

    #[test]
    fn lenient_forms_the_plugin_never_writes_are_accepted_and_documented() {
        // These are accepted by the underlying parser; see `ExportedAt::parse`.
        for text in [
            "2026-01-01 12:00:00Z",
            "2026-01-01t12:00:00z",
            "20260101T120000Z",
            "2026-01-01T12:00:00+25:00",
        ] {
            assert!(ExportedAt::parse(text).is_some(), "{text:?} is accepted");
        }
    }
}
