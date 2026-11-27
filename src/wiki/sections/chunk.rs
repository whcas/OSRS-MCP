//! Splits one section body into bounded chunks of verbatim source.
//!
//! Chunks are substrings of the section body, trimmed of surrounding
//! whitespace, never rewritten. Boundaries prefer paragraph breaks, then line
//! breaks, and cut inside a line only when a single line exceeds the bound.

use std::ops::Range;

/// A candidate piece of text. `cut_before` and `cut_after` mark edges that
/// fall inside a line because the line was too long to keep whole.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Piece {
    range: Range<usize>,
    cut_before: bool,
    cut_after: bool,
}

/// One chunk of a section body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChunkRange {
    /// Byte range within the text that was split.
    pub(super) range: Range<usize>,
    /// Whether an edge of the chunk falls inside a line.
    pub(super) cuts_line: bool,
}

/// Splits `text` into chunks of at most `max_bytes`, in order. Chunks do not
/// overlap, skip whitespace-only text and together cover every other byte.
///
/// `max_bytes` must be at least 4, so that any character fits in a chunk.
pub(super) fn split(text: &str, max_bytes: usize) -> Vec<ChunkRange> {
    let mut chunks = Vec::new();
    let mut current: Option<Piece> = None;
    for piece in pieces(text, max_bytes) {
        current = match current {
            Some(open) if piece.range.end - open.range.start <= max_bytes => Some(Piece {
                range: open.range.start..piece.range.end,
                cut_before: open.cut_before,
                cut_after: piece.cut_after,
            }),
            Some(open) => {
                chunks.extend(finish(text, &open));
                Some(piece)
            }
            None => Some(piece),
        };
    }
    chunks.extend(current.and_then(|open| finish(text, &open)));
    chunks
}

/// Trims the piece to a chunk, or `None` if it holds only whitespace, which
/// happens when a cut lands inside a long run of spaces.
fn finish(text: &str, piece: &Piece) -> Option<ChunkRange> {
    let slice = &text[piece.range.clone()];
    if slice.trim().is_empty() {
        return None;
    }
    let start = piece.range.start + (slice.len() - slice.trim_start().len());
    let end = piece.range.end - (slice.len() - slice.trim_end().len());
    Some(ChunkRange {
        range: start..end,
        cuts_line: piece.cut_before || piece.cut_after,
    })
}

/// Paragraphs, with oversized paragraphs replaced by their lines and oversized
/// lines by slices at character boundaries. Whitespace-only paragraphs are
/// dropped.
fn pieces(text: &str, max_bytes: usize) -> Vec<Piece> {
    let mut pieces = Vec::new();
    for paragraph in paragraphs(text) {
        if paragraph.len() <= max_bytes {
            pieces.push(whole(paragraph));
        } else {
            for line in lines(text, &paragraph) {
                push_line(text, line, max_bytes, &mut pieces);
            }
        }
    }
    pieces
}

const fn whole(range: Range<usize>) -> Piece {
    Piece {
        range,
        cut_before: false,
        cut_after: false,
    }
}

fn push_line(text: &str, line: Range<usize>, max_bytes: usize, pieces: &mut Vec<Piece>) {
    if line.len() <= max_bytes {
        pieces.push(whole(line));
        return;
    }
    let mut start = line.start;
    while start < line.end {
        let end = floor_char_boundary(text, (start + max_bytes).min(line.end));
        pieces.push(Piece {
            range: start..end,
            cut_before: start != line.start,
            cut_after: end != line.end,
        });
        start = end;
    }
}

/// The largest character boundary at or before `index`. The bound is at least
/// four bytes, so this always moves forward.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    (0..=index)
        .rev()
        .find(|&candidate| text.is_char_boundary(candidate))
        .unwrap_or(0)
}

/// Ranges of text separated by blank lines, excluding whitespace-only ones.
/// Each range starts at the paragraph's first line and ends at the end of its
/// last line's content.
fn paragraphs(text: &str) -> Vec<Range<usize>> {
    let mut paragraphs = Vec::new();
    let mut open: Option<Range<usize>> = None;
    let mut offset = 0;
    for raw_line in text.split_inclusive('\n') {
        if raw_line.trim().is_empty() {
            paragraphs.extend(open.take());
        } else {
            let start = open.as_ref().map_or(offset, |range| range.start);
            open = Some(start..offset + without_line_break(raw_line).len());
        }
        offset += raw_line.len();
    }
    paragraphs.extend(open);
    paragraphs
}

/// The lines of `paragraph`, without their line breaks.
fn lines(text: &str, paragraph: &Range<usize>) -> Vec<Range<usize>> {
    let mut offset = paragraph.start;
    text[paragraph.clone()]
        .split_inclusive('\n')
        .map(|line| {
            let range = offset..offset + without_line_break(line).len();
            offset += line.len();
            range
        })
        .collect()
}

fn without_line_break(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(text: &str, max: usize) -> Vec<&str> {
        split(text, max)
            .into_iter()
            .map(|chunk| &text[chunk.range])
            .collect()
    }

    #[test]
    fn short_text_is_one_trimmed_chunk() {
        assert_eq!(chunks("\n\n  Hello world.  \n\n", 100), ["Hello world."]);
    }

    #[test]
    fn empty_and_whitespace_only_text_yield_no_chunks() {
        assert_eq!(chunks("", 100), Vec::<&str>::new());
        assert_eq!(chunks(" \n\t\n  \n", 100), Vec::<&str>::new());
    }

    #[test]
    fn paragraphs_are_packed_together_up_to_the_bound() {
        let text = "aaaa\n\nbbbb\n\ncccc\n\ndddd";
        // "aaaa\n\nbbbb" is 10 bytes; adding "\n\ncccc" would make 16.
        assert_eq!(chunks(text, 10), ["aaaa\n\nbbbb", "cccc\n\ndddd"]);
    }

    #[test]
    fn a_chunk_exactly_at_the_bound_is_allowed_and_one_byte_more_is_not() {
        assert_eq!(chunks("aaaa\n\nbbbb", 10), ["aaaa\n\nbbbb"]);
        assert_eq!(chunks("aaaa\n\nbbbb", 9), ["aaaa", "bbbb"]);
    }

    #[test]
    fn an_oversized_paragraph_is_split_between_lines() {
        let text = "line one\nline two\nline three";
        let result = chunks(text, 18);
        assert_eq!(result, ["line one\nline two", "line three"]);
        assert!(split(text, 18).iter().all(|chunk| !chunk.cuts_line));
    }

    #[test]
    fn a_line_longer_than_the_bound_is_cut_and_flagged() {
        let text = "abcdefghij";
        let result = split(text, 4);
        let parts: Vec<&str> = result
            .iter()
            .map(|chunk| &text[chunk.range.clone()])
            .collect();
        assert_eq!(parts, ["abcd", "efgh", "ij"]);
        assert!(result.iter().all(|chunk| chunk.cuts_line));
    }

    #[test]
    fn a_whole_line_is_not_flagged_when_only_its_neighbour_was_cut() {
        let text = "abcdefghij\nxy";
        let result = split(text, 4);
        let flagged: Vec<(&str, bool)> = result
            .iter()
            .map(|chunk| (&text[chunk.range.clone()], chunk.cuts_line))
            .collect();
        assert_eq!(
            flagged,
            [("abcd", true), ("efgh", true), ("ij", true), ("xy", false)]
        );
    }

    #[test]
    fn cuts_fall_on_character_boundaries() {
        let text = "é☕🐟é☕🐟é☕🐟";
        for max in 4..12 {
            let result = split(text, max);
            let joined: String = result
                .iter()
                .map(|chunk| &text[chunk.range.clone()])
                .collect();
            assert_eq!(joined, text, "max {max}");
            assert!(
                result.iter().all(|chunk| chunk.range.len() <= max),
                "max {max}"
            );
        }
    }

    #[test]
    fn every_chunk_respects_the_bound_and_nothing_but_whitespace_is_lost() {
        let text =
            "alpha beta\n\n  gamma  \ndelta epsilon zeta\n\n\n\neta\ntheta iota kappa lambda";
        for max in 4..60 {
            let result = split(text, max);
            let kept: String = result
                .iter()
                .flat_map(|chunk| text[chunk.range.clone()].chars())
                .filter(|c| !c.is_whitespace())
                .collect();
            let expected: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            assert_eq!(kept, expected, "max {max}");
            assert!(
                result.iter().all(|chunk| chunk.range.len() <= max),
                "max {max}"
            );
            assert!(
                result
                    .windows(2)
                    .all(|pair| pair[0].range.end <= pair[1].range.start),
                "max {max}"
            );
        }
    }

    #[test]
    fn a_cut_inside_a_long_run_of_spaces_does_not_produce_a_blank_chunk() {
        let text = format!("ab{}cd", " ".repeat(20));
        let result = split(&text, 4);
        let parts: Vec<&str> = result
            .iter()
            .map(|chunk| &text[chunk.range.clone()])
            .collect();
        assert_eq!(parts, ["ab", "cd"]);
    }

    #[test]
    fn splitting_is_deterministic() {
        let text = "one\n\ntwo two two\n\nthree three three three";
        assert_eq!(split(text, 12), split(text, 12));
    }
}
