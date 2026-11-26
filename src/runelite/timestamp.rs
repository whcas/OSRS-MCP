//! The `exported_at` timestamp.
//!
//! The plugin writes `OffsetDateTime.now().toString()`, so the text is
//! ISO-8601 with an offset, seconds and fractions omitted when zero, and a
//! fraction printed in groups of three digits. This module accepts exactly
//! those forms. It is hand-written to avoid a date-time dependency for one
//! format; the original text is always kept next to the parsed instant.

/// A parsed `exported_at` value with its original text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedAt {
    text: String,
    unix_seconds: i64,
    nanos: u32,
}

impl ExportedAt {
    /// Parses the plugin's timestamp text, or `None` if it is not one of the
    /// accepted forms. Validation is strict: calendar ranges are checked and
    /// only uppercase `T` and `Z` are accepted.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut cursor = Cursor::new(text);
        let year = cursor.number(4)?;
        cursor.expect(b'-')?;
        let month = cursor.number(2)?;
        cursor.expect(b'-')?;
        let day = cursor.number(2)?;
        cursor.expect(b'T')?;
        let hour = cursor.number(2)?;
        cursor.expect(b':')?;
        let minute = cursor.number(2)?;
        let (second, nanos) = if cursor.eat(b':') {
            let second = cursor.number(2)?;
            let nanos = if cursor.eat(b'.') {
                cursor.fraction()?
            } else {
                0
            };
            (second, nanos)
        } else {
            (0, 0)
        };
        let offset_seconds = cursor.offset()?;
        if !cursor.at_end() {
            return None;
        }

        if !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return None;
        }
        let local =
            days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
        Some(Self {
            text: text.to_owned(),
            unix_seconds: local - offset_seconds,
            nanos,
        })
    }

    /// The original text, exactly as exported.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Seconds since the Unix epoch (UTC), ignoring the fraction.
    #[must_use]
    pub const fn unix_seconds(&self) -> i64 {
        self.unix_seconds
    }

    /// The fractional second, in nanoseconds.
    #[must_use]
    pub const fn nanos(&self) -> u32 {
        self.nanos
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    const fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            position: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn eat(&mut self, expected: u8) -> bool {
        let found = self.peek() == Some(expected);
        if found {
            self.position += 1;
        }
        found
    }

    fn expect(&mut self, expected: u8) -> Option<()> {
        self.eat(expected).then_some(())
    }

    const fn at_end(&self) -> bool {
        self.position == self.bytes.len()
    }

    /// Exactly `digits` ASCII digits.
    fn number(&mut self, digits: usize) -> Option<i64> {
        let end = self.position.checked_add(digits)?;
        let slice = self.bytes.get(self.position..end)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.position = end;
        Some(
            slice
                .iter()
                .fold(0, |value, digit| value * 10 + i64::from(digit - b'0')),
        )
    }

    /// One to nine fraction digits, scaled to nanoseconds.
    fn fraction(&mut self) -> Option<u32> {
        let start = self.position;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.position += 1;
        }
        let digits = self.bytes.get(start..self.position)?;
        if digits.is_empty() || digits.len() > 9 {
            return None;
        }
        let mut nanos = 0_u32;
        for index in 0..9 {
            let digit = digits.get(index).map_or(0, |digit| u32::from(digit - b'0'));
            nanos = nanos * 10 + digit;
        }
        Some(nanos)
    }

    /// `Z`, or `+HH:MM` / `-HH:MM` with optional `:SS`, as seconds east of UTC.
    fn offset(&mut self) -> Option<i64> {
        if self.eat(b'Z') {
            return Some(0);
        }
        let sign = if self.eat(b'+') {
            1
        } else if self.eat(b'-') {
            -1
        } else {
            return None;
        };
        let hours = self.number(2)?;
        self.expect(b':')?;
        let minutes = self.number(2)?;
        let seconds = if self.eat(b':') { self.number(2)? } else { 0 };
        let total = hours * 3_600 + minutes * 60 + seconds;
        // Offsets are limited to +/-18:00:00; minutes and seconds to 0..=59.
        (minutes <= 59 && seconds <= 59 && total <= 18 * 3_600).then_some(sign * total)
    }
}

const fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

const fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap_year(year) => 29,
        _ => 28,
    }
}

/// Days from 1970-01-01 to the given civil date (proleptic Gregorian).
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
