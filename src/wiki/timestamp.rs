//! Revision timestamps as exported by `MediaWiki`.

use jiff::Timestamp;
use thiserror::Error;

use crate::domain::UnixSeconds;

/// The text was not a whole-second UTC timestamp of the form
/// `YYYY-MM-DDTHH:MM:SSZ`, or named a date that does not exist or precedes the
/// Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("timestamp is not a valid YYYY-MM-DDTHH:MM:SSZ date from 1970 onwards")]
pub struct InvalidTimestamp;

/// A revision time: the exact text the export carried and the instant it
/// denotes.
///
/// `MediaWiki` exports whole-second ISO 8601 timestamps in UTC, so
/// [`UnixSeconds`] is enough for ordering and staleness. The original text is
/// kept for provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionTimestamp {
    seconds: UnixSeconds,
    text: String,
}

impl RevisionTimestamp {
    /// Parses a timestamp of exactly the form `YYYY-MM-DDTHH:MM:SSZ`.
    ///
    /// `jiff` does the calendar arithmetic, but it also accepts offsets,
    /// fractions, lower-case separators and leap seconds, which an export never
    /// contains. Those are rejected by requiring whole seconds and text that is
    /// exactly what `jiff` prints for the instant it parsed. Dates before 1970 are rejected
    /// too.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidTimestamp`] if the text has another shape or names a
    /// date or time that does not exist.
    pub fn parse(text: &str) -> Result<Self, InvalidTimestamp> {
        let instant: Timestamp = text.parse().map_err(|_| InvalidTimestamp)?;
        if instant.subsec_nanosecond() != 0 || instant.to_string() != text {
            return Err(InvalidTimestamp);
        }
        let seconds = UnixSeconds::try_from(instant.as_second()).map_err(|_| InvalidTimestamp)?;
        Ok(Self {
            seconds,
            text: text.to_owned(),
        })
    }

    /// The instant as whole seconds since the Unix epoch, UTC.
    #[must_use]
    pub const fn seconds(&self) -> UnixSeconds {
        self.seconds
    }

    /// The timestamp text exactly as exported.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(text: &str) -> Result<u64, InvalidTimestamp> {
        RevisionTimestamp::parse(text).map(|timestamp| timestamp.seconds().0)
    }

    #[test]
    fn converts_known_instants() {
        assert_eq!(seconds("1970-01-01T00:00:00Z"), Ok(0));
        assert_eq!(seconds("1999-12-31T23:59:59Z"), Ok(946_684_799));
        assert_eq!(seconds("2000-02-29T12:00:00Z"), Ok(951_825_600));
        assert_eq!(seconds("2026-11-27T10:24:37Z"), Ok(1_795_775_077));
        assert_eq!(seconds("2100-03-01T00:00:00Z"), Ok(4_107_542_400));
    }

    #[test]
    fn keeps_the_original_text() {
        let timestamp = RevisionTimestamp::parse("2026-11-27T10:24:37Z").unwrap();
        assert_eq!(timestamp.as_str(), "2026-11-27T10:24:37Z");
    }

    #[test]
    fn leap_days_exist_only_in_leap_years() {
        assert!(seconds("2024-02-29T00:00:00Z").is_ok());
        assert!(seconds("2000-02-29T00:00:00Z").is_ok());
        assert_eq!(seconds("2023-02-29T00:00:00Z"), Err(InvalidTimestamp));
        assert_eq!(seconds("2100-02-29T00:00:00Z"), Err(InvalidTimestamp));
    }

    #[test]
    fn rejects_dates_and_times_that_do_not_exist() {
        for text in [
            "2026-00-10T00:00:00Z",
            "2026-13-10T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-04-00T00:00:00Z",
            "2026-04-10T24:00:00Z",
            "2026-04-10T00:60:00Z",
            "2026-04-10T00:00:60Z",
            "1969-12-31T23:59:59Z",
        ] {
            assert_eq!(seconds(text), Err(InvalidTimestamp), "{text}");
        }
    }

    #[test]
    fn rejects_other_shapes() {
        for text in [
            "",
            "2026-11-27",
            "2026-11-27T10:24:37",
            "2026-11-27 10:24:37Z",
            "2026-11-27t10:24:37Z",
            "2026-11-27T10:24:37z",
            "2026-11-27T10:24:37+00:00",
            "2026-11-27T10:24:37.5Z",
            "2026-11-27T10:24:37ZZ",
            "+026-11-27T10:24:37Z",
            "2026-11-27T1O:24:37Z",
            "２０２６-11-27T10:24:37Z",
        ] {
            assert_eq!(seconds(text), Err(InvalidTimestamp), "{text}");
        }
    }
}
