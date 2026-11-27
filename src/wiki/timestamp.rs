//! Revision timestamps as exported by `MediaWiki`.

use thiserror::Error;

use crate::domain::UnixSeconds;

const SECONDS_PER_DAY: u64 = 86_400;
/// Length of `YYYY-MM-DDTHH:MM:SSZ`.
const TIMESTAMP_LEN: usize = 20;

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
    /// Only that form is accepted: no offsets, fractions or lower-case
    /// separators. Leap seconds (`:60`) are rejected, as are dates before 1970.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidTimestamp`] if the text has another shape or names a
    /// date or time that does not exist.
    pub fn parse(text: &str) -> Result<Self, InvalidTimestamp> {
        let seconds = parse_seconds(text.as_bytes()).ok_or(InvalidTimestamp)?;
        Ok(Self {
            seconds: UnixSeconds(seconds),
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

fn parse_seconds(text: &[u8]) -> Option<u64> {
    let &[
        y0,
        y1,
        y2,
        y3,
        b'-',
        mo0,
        mo1,
        b'-',
        d0,
        d1,
        b'T',
        h0,
        h1,
        b':',
        mi0,
        mi1,
        b':',
        s0,
        s1,
        b'Z',
    ] = <&[u8; TIMESTAMP_LEN]>::try_from(text).ok()?
    else {
        return None;
    };
    let year = digits(&[y0, y1, y2, y3])?;
    let month = digits(&[mo0, mo1])?;
    let day = digits(&[d0, d1])?;
    let hour = digits(&[h0, h1])?;
    let minute = digits(&[mi0, mi1])?;
    let second = digits(&[s0, s1])?;

    let valid = year >= 1970
        && (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour < 24
        && minute < 60
        && second < 60;
    valid.then(|| {
        days_from_civil(year, month, day) * SECONDS_PER_DAY + hour * 3600 + minute * 60 + second
    })
}

/// Decimal value of ASCII digits, or `None` if any byte is not a digit.
fn digits(bytes: &[u8]) -> Option<u64> {
    bytes.iter().try_fold(0_u64, |value, byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + u64::from(byte - b'0'))
    })
}

const fn is_leap_year(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

const fn days_in_month(year: u64, month: u64) -> u64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the given proleptic Gregorian date, for years from
/// 1970. This is Howard Hinnant's `days_from_civil`, restricted to dates where
/// every intermediate value is non-negative.
const fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    // March-based year, so that the leap day is the last day of the year.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let month_from_march = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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
