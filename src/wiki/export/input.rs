//! A byte-counting reader that enforces the input and page size limits.

use std::io::{self, BufRead, Read};

use super::error::LimitKind;

/// Wraps a [`BufRead`] and refuses to hand out more bytes than the limits
/// allow.
///
/// The XML parser buffers one event at a time, so a single huge element would
/// otherwise be held in memory in full before any size could be checked.
/// Counting happens in [`BufRead::consume`], and [`BufRead::fill_buf`] never
/// exposes bytes beyond the tightest active limit, so the parser can consume at
/// most the limit and fails with an I/O error on the next read instead.
/// Reaching exactly the end of the data at the limit is not an error.
///
/// The parser reads no further ahead than it consumes, so the count is exact.
#[derive(Debug)]
pub(super) struct BoundedInput<R> {
    inner: R,
    consumed: u64,
    max_input: u64,
    page: Option<PageWindow>,
    exceeded: Option<(LimitKind, u64)>,
}

#[derive(Debug, Clone, Copy)]
struct PageWindow {
    start: u64,
    max: u64,
}

impl<R: BufRead> BoundedInput<R> {
    pub(super) const fn new(inner: R, max_input: u64) -> Self {
        Self {
            inner,
            consumed: 0,
            max_input,
            page: None,
            exceeded: None,
        }
    }

    /// Starts counting a page from the current position, with its own limit.
    pub(super) const fn begin_page(&mut self, max_bytes: u64) {
        self.page = Some(PageWindow {
            start: self.consumed,
            max: max_bytes,
        });
    }

    pub(super) const fn end_page(&mut self) {
        self.page = None;
    }

    /// The limit that stopped the last read, with its configured maximum.
    pub(super) const fn exceeded(&self) -> Option<(LimitKind, u64)> {
        self.exceeded
    }

    /// Bytes still allowed, and the limit that bounds them. The page limit wins
    /// a tie, because it is the more specific.
    const fn allowance(&self) -> (u64, LimitKind, u64) {
        let input_left = self.max_input.saturating_sub(self.consumed);
        match self.page {
            Some(page) => {
                let page_left = page.max.saturating_sub(self.consumed - page.start);
                if page_left <= input_left {
                    (page_left, LimitKind::Page, page.max)
                } else {
                    (input_left, LimitKind::Input, self.max_input)
                }
            }
            None => (input_left, LimitKind::Input, self.max_input),
        }
    }
}

impl<R: BufRead> Read for BoundedInput<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let count = available.len().min(buf.len());
        buf[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

impl<R: BufRead> BufRead for BoundedInput<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let (allowed, limit, max) = self.allowance();
        let buffered = self.inner.fill_buf()?.len();
        if buffered > 0 && allowed == 0 {
            self.exceeded = Some((limit, max));
            return Err(io::Error::other("size limit reached"));
        }
        let visible = usize::try_from(allowed).map_or(buffered, |allowed| buffered.min(allowed));
        Ok(&self.inner.fill_buf()?[..visible])
    }

    fn consume(&mut self, amount: usize) {
        self.inner.consume(amount);
        self.consumed = self
            .consumed
            .saturating_add(u64::try_from(amount).unwrap_or(u64::MAX));
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn read_all(input: &mut BoundedInput<Cursor<Vec<u8>>>) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        input.read_to_end(&mut out)?;
        Ok(out)
    }

    fn bounded(data: &[u8], max_input: u64) -> BoundedInput<Cursor<Vec<u8>>> {
        BoundedInput::new(Cursor::new(data.to_vec()), max_input)
    }

    #[test]
    fn input_exactly_at_the_limit_is_read_in_full() {
        let mut input = bounded(b"abcdef", 6);
        assert_eq!(read_all(&mut input).unwrap(), b"abcdef");
        assert_eq!(input.exceeded(), None);
    }

    #[test]
    fn one_byte_over_the_input_limit_fails_and_names_the_limit() {
        let mut input = bounded(b"abcdefg", 6);
        assert!(read_all(&mut input).is_err());
        assert_eq!(input.exceeded(), Some((LimitKind::Input, 6)));
    }

    #[test]
    fn nothing_beyond_the_limit_is_ever_exposed() {
        let mut input = bounded(b"abcdefghij", 4);
        assert_eq!(input.fill_buf().unwrap(), b"abcd");
    }

    #[test]
    fn page_limit_counts_from_where_the_page_began() {
        let mut input = bounded(b"0123456789", 100);
        let mut skipped = [0_u8; 3];
        input.read_exact(&mut skipped).unwrap();

        input.begin_page(4);
        let mut page = [0_u8; 4];
        input.read_exact(&mut page).unwrap();
        assert_eq!(&page, b"3456");
        assert_eq!(input.exceeded(), None);

        let mut one_more = [0_u8; 1];
        assert!(input.read(&mut one_more).is_err());
        assert_eq!(input.exceeded(), Some((LimitKind::Page, 4)));
    }

    #[test]
    fn ending_a_page_lifts_its_limit() {
        let mut input = bounded(b"0123456789", 100);
        input.begin_page(2);
        let mut page = [0_u8; 2];
        input.read_exact(&mut page).unwrap();
        input.end_page();
        assert_eq!(read_all(&mut input).unwrap(), b"23456789");
    }

    #[test]
    fn the_input_limit_still_applies_inside_a_page() {
        let mut input = bounded(b"0123456789", 5);
        input.begin_page(50);
        assert!(read_all(&mut input).is_err());
        assert_eq!(input.exceeded(), Some((LimitKind::Input, 5)));
    }

    #[test]
    fn the_page_limit_wins_a_tie_with_the_input_limit() {
        let mut input = bounded(b"0123456789", 4);
        input.begin_page(4);
        assert!(read_all(&mut input).is_err());
        assert_eq!(input.exceeded(), Some((LimitKind::Page, 4)));
    }

    #[test]
    fn a_page_that_ends_exactly_at_the_end_of_data_is_not_over_the_limit() {
        let mut input = bounded(b"abcd", 100);
        input.begin_page(4);
        assert_eq!(read_all(&mut input).unwrap(), b"abcd");
        assert_eq!(input.exceeded(), None);
    }
}
