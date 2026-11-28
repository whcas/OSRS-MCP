//! Splits one section body into bounded chunks of verbatim source.
//!
//! Chunks are substrings of the section body, trimmed of surrounding
//! whitespace, never rewritten. Boundaries prefer, in order: paragraph breaks,
//! breaks between blocks (see [`structure`]: lines, table rows, multi-line
//! template calls), breaks between the lines of one block, and finally cuts
//! inside a line, which happen only when a single line exceeds the bound.
//!
//! A caveat stays with the claim before it whenever the two fit together.
//! Every boundary that still damages its surroundings is reported as a
//! [`Loss`] on the chunks on both sides of it.

use std::ops::Range;

use super::Loss;
use super::structure::{self, Block, Paragraph};

/// The [`Loss`]es caused by one boundary between pieces, as a small set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Cut(u8);

impl Cut {
    const NONE: Self = Self(0);
    /// Every loss, in [`Loss`] order. [`Cut::bit`] must name each one, so the
    /// compiler flags a new variant missing from both.
    const ALL: [Loss; 4] = [
        Loss::CutLine,
        Loss::SplitMarkup,
        Loss::SplitTable,
        Loss::SplitCaveat,
    ];

    const fn bit(loss: Loss) -> u8 {
        match loss {
            Loss::CutLine => 1,
            Loss::SplitMarkup => 2,
            Loss::SplitTable => 4,
            Loss::SplitCaveat => 8,
        }
    }

    const fn of(loss: Loss) -> Self {
        Self(Self::bit(loss))
    }

    /// This cut if `condition` holds, otherwise no cut.
    const fn when(self, condition: bool) -> Self {
        if condition { self } else { Self::NONE }
    }

    const fn and(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    const fn has(self, loss: Loss) -> bool {
        self.0 & Self::bit(loss) != 0
    }

    /// The losses of a chunk whose edges are `self` and `other`.
    fn losses(self, other: Self) -> Vec<Loss> {
        let both = self.and(other);
        Self::ALL
            .into_iter()
            .filter(|loss| both.has(*loss))
            .collect()
    }
}

/// A candidate piece of text and the boundary before it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Piece {
    range: Range<usize>,
    before: Cut,
}

/// One chunk of a section body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChunkRange {
    /// Byte range within the text that was split.
    pub(super) range: Range<usize>,
    /// What the chunk's edges damage, without repeats, in [`Loss`] order.
    pub(super) losses: Vec<Loss>,
}

/// A chunk being filled.
struct Open {
    range: Range<usize>,
    before: Cut,
}

/// Splits `text` into chunks of at most `max_bytes`, in order. Chunks do not
/// overlap, skip whitespace-only text and together cover every other byte.
///
/// `max_bytes` must be at least 4, so that any character fits in a chunk.
pub(super) fn split(text: &str, max_bytes: usize) -> Vec<ChunkRange> {
    let mut chunks = Vec::new();
    let mut current: Option<Open> = None;
    for piece in pieces(text, max_bytes) {
        current = match current {
            Some(mut open) if piece.range.end - open.range.start <= max_bytes => {
                open.range.end = piece.range.end;
                Some(open)
            }
            Some(open) => {
                chunks.extend(finish(text, &open, piece.before));
                Some(Open::from(piece))
            }
            None => Some(Open::from(piece)),
        };
    }
    chunks.extend(current.and_then(|open| finish(text, &open, Cut::NONE)));
    chunks
}

impl From<Piece> for Open {
    fn from(piece: Piece) -> Self {
        Self {
            range: piece.range,
            before: piece.before,
        }
    }
}

/// Trims the chunk, or returns `None` if it holds only whitespace, which
/// happens when a cut lands inside a long run of spaces. `after` is the
/// boundary that ends the chunk.
fn finish(text: &str, open: &Open, after: Cut) -> Option<ChunkRange> {
    let slice = &text[open.range.clone()];
    if slice.trim().is_empty() {
        return None;
    }
    let start = open.range.start + (slice.len() - slice.trim_start().len());
    let end = open.range.end - (slice.len() - slice.trim_end().len());
    Some(ChunkRange {
        range: start..end,
        losses: open.before.losses(after),
    })
}

/// The pieces of `text` that chunks are packed from, with caveats already
/// joined to their claims where the two fit in `max_bytes`.
fn pieces(text: &str, max_bytes: usize) -> Vec<Piece> {
    let mut pieces = Vec::new();
    for paragraph in structure::paragraphs(text) {
        push_paragraph(text, &paragraph, max_bytes, &mut pieces);
    }
    let mut pieces = join_caveats(pieces, max_bytes);
    if let Some(first) = pieces.first_mut() {
        first.before = Cut::NONE;
    }
    pieces
}

/// A paragraph that fits is one piece. Otherwise each table that fits, and each
/// other block, is.
fn push_paragraph(text: &str, paragraph: &Paragraph, max_bytes: usize, pieces: &mut Vec<Piece>) {
    if paragraph.range.len() <= max_bytes {
        pieces.push(Piece {
            range: paragraph.range.clone(),
            before: Cut::of(Loss::SplitCaveat)
                .when(paragraph.blocks.first().is_some_and(|block| block.caveat)),
        });
        return;
    }
    // A run is the blocks of one table, or a single block outside tables.
    for run in paragraph.blocks.chunk_by(in_same_table) {
        let range = run[0].range.start..run[run.len() - 1].range.end;
        if range.len() <= max_bytes {
            pieces.push(Piece {
                range,
                before: Cut::of(Loss::SplitCaveat).when(run[0].caveat),
            });
            continue;
        }
        for (index, block) in run.iter().enumerate() {
            let before = Cut::of(Loss::SplitCaveat)
                .when(block.caveat)
                .and(Cut::of(Loss::SplitTable).when(index > 0));
            push_block(text, block, before, max_bytes, pieces);
        }
    }
}

fn in_same_table(earlier: &Block, later: &Block) -> bool {
    earlier.table.is_some() && earlier.table == later.table
}

/// A block that fits is one piece. Otherwise its lines are.
fn push_block(text: &str, block: &Block, before: Cut, max_bytes: usize, pieces: &mut Vec<Piece>) {
    if block.range.len() <= max_bytes {
        pieces.push(Piece {
            range: block.range.clone(),
            before,
        });
        return;
    }
    for (index, line) in structure::lines(text, block).into_iter().enumerate() {
        let before = if index == 0 {
            before
        } else {
            Cut::of(Loss::SplitMarkup)
                .when(line.continues_markup)
                .and(Cut::of(Loss::SplitTable).when(block.table.is_some()))
        };
        push_line(
            text,
            line.range,
            before,
            block.table.is_some(),
            max_bytes,
            pieces,
        );
    }
}

/// A line that fits is one piece. Otherwise it is cut into slices at character
/// boundaries.
fn push_line(
    text: &str,
    line: Range<usize>,
    before: Cut,
    in_table: bool,
    max_bytes: usize,
    pieces: &mut Vec<Piece>,
) {
    if line.len() <= max_bytes {
        pieces.push(Piece {
            range: line,
            before,
        });
        return;
    }
    let mut start = line.start;
    while start < line.end {
        let end = text.floor_char_boundary((start + max_bytes).min(line.end));
        let before = if start == line.start {
            before
        } else {
            Cut::of(Loss::CutLine).and(Cut::of(Loss::SplitTable).when(in_table))
        };
        pieces.push(Piece {
            range: start..end,
            before,
        });
        start = end;
    }
}

/// Merges each caveat piece into the piece before it, as long as the result
/// fits. A caveat that does not fit keeps the cut that separates it.
fn join_caveats(pieces: Vec<Piece>, max_bytes: usize) -> Vec<Piece> {
    let mut joined: Vec<Piece> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        match joined.last_mut() {
            Some(claim)
                if piece.before.has(Loss::SplitCaveat)
                    && piece.range.end - claim.range.start <= max_bytes =>
            {
                claim.range.end = piece.range.end;
            }
            _ => joined.push(piece),
        }
    }
    joined
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
        assert!(split(text, 18).iter().all(|chunk| chunk.losses.is_empty()));
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
        assert!(result.iter().all(|chunk| chunk.losses == [Loss::CutLine]));
    }

    #[test]
    fn a_whole_line_is_not_flagged_when_only_its_neighbour_was_cut() {
        let text = "abcdefghij\nxy";
        let result = split(text, 4);
        let flagged: Vec<(&str, bool)> = result
            .iter()
            .map(|chunk| (&text[chunk.range.clone()], chunk.losses == [Loss::CutLine]))
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

    fn labelled(text: &str, max: usize) -> Vec<(&str, Vec<Loss>)> {
        split(text, max)
            .into_iter()
            .map(|chunk| (&text[chunk.range], chunk.losses))
            .collect()
    }

    #[test]
    fn a_table_that_fits_is_one_chunk_without_losses() {
        let text = "{|\n! Name !! Level\n|-\n| Axe || 1\n|-\n| Pick || 5\n|}";
        assert_eq!(labelled(text, text.len()), [(text, vec![])]);
    }

    #[test]
    fn a_long_table_is_split_between_rows_and_both_sides_say_so() {
        let text = "{|\n! Name !! Level\n|-\n| Axe || 1\n|-\n| Pick || 5\n|}";
        // Header block, then two rows of 15 and 17 bytes.
        let result = labelled(text, 20);
        assert_eq!(
            result,
            [
                ("{|\n! Name !! Level", vec![Loss::SplitTable]),
                ("|-\n| Axe || 1", vec![Loss::SplitTable]),
                ("|-\n| Pick || 5\n|}", vec![Loss::SplitTable]),
            ]
        );
    }

    #[test]
    fn text_around_a_table_does_not_count_as_a_split_table() {
        let text = "intro line\n{|\n|-\n| a\n|}\noutro line";
        let result = labelled(text, 14);
        assert_eq!(
            result,
            [
                ("intro line", vec![]),
                ("{|\n|-\n| a\n|}", vec![]),
                ("outro line", vec![]),
            ]
        );
    }

    #[test]
    fn list_items_are_never_cut_when_they_fit() {
        let text = "* alpha item\n* beta item\n** nested item\n# gamma item";
        for max in 14..text.len() {
            for chunk in labelled(text, max) {
                assert!(chunk.1.is_empty(), "max {max}: {chunk:?}");
                assert!(
                    chunk
                        .0
                        .lines()
                        .all(|line| text.lines().any(|item| item == line)),
                    "max {max}: {chunk:?}"
                );
            }
        }
    }

    #[test]
    fn a_multiline_template_that_fits_stays_whole() {
        let text = "before text\n{{Box\n|a=1\n|b=2\n}}\nafter text";
        // The call is 19 bytes; greedy line packing alone would cut it.
        let result = labelled(text, 20);
        assert_eq!(
            result,
            [
                ("before text", vec![]),
                ("{{Box\n|a=1\n|b=2\n}}", vec![]),
                ("after text", vec![]),
            ]
        );
    }

    #[test]
    fn a_template_longer_than_the_bound_is_split_between_lines_and_flagged() {
        let text = "{{Box\n|a=1\n|b=2\n}}";
        assert_eq!(
            labelled(text, 10),
            [
                ("{{Box\n|a=1", vec![Loss::SplitMarkup]),
                ("|b=2\n}}", vec![Loss::SplitMarkup]),
            ]
        );
    }

    #[test]
    fn a_cut_inside_a_line_of_a_long_table_row_reports_both_losses() {
        let text = "{|\n|-\n| aaaaaaaaaaaaaaaa\n|}";
        let result = labelled(text, 8);
        assert!(
            result
                .iter()
                .any(|(_, losses)| losses.contains(&Loss::CutLine))
        );
        assert!(
            result
                .iter()
                .all(|(_, losses)| losses.contains(&Loss::SplitTable))
        );
    }

    #[test]
    fn a_caveat_stays_with_its_claim_when_they_fit_together() {
        let text = "intro line one\nclaim claim\nNote: caveat";
        // Without joining, the first chunk would end after "claim claim".
        assert_eq!(
            labelled(text, 26),
            [
                ("intro line one", vec![]),
                ("claim claim\nNote: caveat", vec![]),
            ]
        );
    }

    #[test]
    fn a_caveat_paragraph_stays_with_the_paragraph_before_it() {
        let text = "filler filler\n\nclaim\n\nHowever, it varies";
        assert_eq!(
            labelled(text, 28),
            [
                ("filler filler", vec![]),
                ("claim\n\nHowever, it varies", vec![]),
            ]
        );
    }

    #[test]
    fn a_caveat_that_cannot_fit_is_split_and_both_chunks_say_so() {
        let text = "claim claim\nNote: caveat";
        assert_eq!(
            labelled(text, 12),
            [
                ("claim claim", vec![Loss::SplitCaveat]),
                ("Note: caveat", vec![Loss::SplitCaveat]),
            ]
        );
    }

    #[test]
    fn a_references_tag_is_not_a_footnote_caveat_but_a_ref_tag_is() {
        let claim = "This claim line is thirty b.";
        assert_eq!(
            labelled(&format!("{claim}\n<references />"), 40),
            [(claim, vec![]), ("<references />", vec![])]
        );
        assert_eq!(
            labelled(&format!("{claim}\n<ref>x</ref>"), 40),
            [
                (claim, vec![Loss::SplitCaveat]),
                ("<ref>x</ref>", vec![Loss::SplitCaveat]),
            ]
        );
    }

    #[test]
    fn a_chain_of_caveats_joins_as_far_as_the_bound_allows() {
        let text = "claim\nNote: a\nHowever, b\nNote: c";
        let result = labelled(text, 24);
        assert_eq!(result[0].0, "claim\nNote: a\nHowever, b");
        assert_eq!(result[0].1, [Loss::SplitCaveat]);
        assert_eq!(result[1], ("Note: c", vec![Loss::SplitCaveat]));
    }

    #[test]
    fn a_leading_caveat_has_no_claim_to_be_split_from() {
        assert_eq!(labelled("Note: first\nsecond", 12)[0].1, []);
    }

    #[test]
    fn mixed_markup_is_covered_in_order_within_the_bound_at_every_size() {
        let text = "intro\n\n* one\n* two\nNote: aside\n\n{|\n! H\n|-\n| a\n| b\n|-\n| c\n|}\n\n\
            {{Box\n|k=v\n|m={{In|x}}\n}}\nHowever, é☕🐟 end\n<!-- c\n\nc -->";
        let expected: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        for max in 4..text.len() + 2 {
            let result = split(text, max);
            let kept: String = result
                .iter()
                .flat_map(|chunk| text[chunk.range.clone()].chars())
                .filter(|c| !c.is_whitespace())
                .collect();
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
            assert!(
                result
                    .iter()
                    .all(|chunk| chunk.losses.windows(2).all(|w| w[0] < w[1])),
                "max {max}"
            );
            assert_eq!(result, split(text, max), "max {max}");
        }
    }
}
