//! Groups the lines of a section body into blocks that should stay whole.
//!
//! A chunk boundary inside a table row, a multi-line template call or a
//! caveat's claim damages the text around it, so chunking prefers to cut
//! between blocks. This module only finds the blocks; it neither expands nor
//! interprets markup beyond what that needs.
//!
//! - A **block** is one line, extended over following lines while a comment,
//!   literal tag or template call opened on it is still open, and, inside a
//!   table, over the lines of one row (`|-` starts the next row).
//! - A **paragraph** is a run of blocks without a blank line between them. A
//!   blank line inside a table or a construct does not end the paragraph.
//! - A block is a **caveat** when its first line opens with a note, a
//!   "however" or a footnote definition; see [`is_caveat`].
//!
//! Known limits, shared with the heading scanner: a table opener or closer
//! inside a template or comment is not seen, and an unclosed table runs to the
//! end of the section.

use std::ops::Range;

use super::scan::{self, ScanState};

/// A run of lines that is cut only when it cannot fit in one chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Block {
    /// From the start of the first line to the end of the last line's content,
    /// without its line break.
    pub(super) range: Range<usize>,
    /// The ordinal of the table the block belongs to, counted from zero over
    /// the text, or `None` outside tables.
    pub(super) table: Option<usize>,
    /// Whether the block qualifies a claim made before it.
    pub(super) caveat: bool,
}

/// Blocks with no blank line between them. Never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Paragraph {
    pub(super) range: Range<usize>,
    pub(super) blocks: Vec<Block>,
}

/// One line of a block, and whether it starts inside a comment, literal tag or
/// template call that an earlier line of the block opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Line {
    pub(super) range: Range<usize>,
    pub(super) continues_markup: bool,
}

/// The paragraphs of `text`, in order. Whitespace-only lines separate
/// paragraphs and belong to none.
pub(super) fn paragraphs(text: &str) -> Vec<Paragraph> {
    let mut builder = Builder::default();
    let mut state = ScanState::new();
    let mut offset = 0;
    for raw_line in text.split_inclusive('\n') {
        let line = without_line_break(raw_line);
        builder.add_line(offset..offset + line.len(), line, state.is_top_level());
        scan::advance(&mut state, line);
        offset += raw_line.len();
    }
    builder.finish()
}

/// The lines of `block`, in order.
pub(super) fn lines(text: &str, block: &Block) -> Vec<Line> {
    let mut state = ScanState::new();
    let mut offset = block.range.start;
    text[block.range.clone()]
        .split_inclusive('\n')
        .map(|raw_line| {
            let line = without_line_break(raw_line);
            let range = offset..offset + line.len();
            let continues_markup = !state.is_top_level();
            scan::advance(&mut state, line);
            offset += raw_line.len();
            Line {
                range,
                continues_markup,
            }
        })
        .collect()
}

fn without_line_break(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

/// What a line inside a table does to the table.
enum TableLine {
    /// Starts the next row of the outermost table.
    Row,
    /// Opens a nested table.
    Open,
    /// Closes the innermost table.
    Close,
    Other,
}

impl TableLine {
    fn classify(line: &str) -> Self {
        let line = line.trim_start();
        if line.starts_with("|-") {
            Self::Row
        } else if line.starts_with("{|") {
            Self::Open
        } else if line.starts_with("|}") {
            Self::Close
        } else {
            Self::Other
        }
    }
}

#[derive(Default)]
struct Builder {
    paragraphs: Vec<Paragraph>,
    open: Option<Paragraph>,
    /// How many tables are open, counting nested ones.
    table_depth: usize,
    /// How many tables have started so far.
    tables_started: usize,
}

impl Builder {
    fn add_line(&mut self, range: Range<usize>, line: &str, top_level: bool) {
        if !top_level {
            self.extend(range, line);
        } else if self.table_depth > 0 {
            self.add_table_line(range, line);
        } else if line.trim().is_empty() {
            self.close_paragraph();
        } else if line.trim_start().starts_with("{|") {
            self.table_depth = 1;
            self.tables_started += 1;
            self.start_block(range, line);
        } else {
            self.start_block(range, line);
        }
    }

    fn add_table_line(&mut self, range: Range<usize>, line: &str) {
        match TableLine::classify(line) {
            TableLine::Row if self.table_depth == 1 => self.start_block(range, line),
            TableLine::Open => {
                self.table_depth += 1;
                self.extend(range, line);
            }
            TableLine::Close => {
                self.table_depth = self.table_depth.saturating_sub(1);
                self.extend(range, line);
            }
            TableLine::Row | TableLine::Other => self.extend(range, line),
        }
    }

    /// Continues the last block over this line. An open construct or table
    /// always has a block; starting one otherwise only keeps the result well
    /// formed.
    fn extend(&mut self, range: Range<usize>, line: &str) {
        let Some(paragraph) = self.open.as_mut() else {
            return self.start_block(range, line);
        };
        paragraph.range.end = range.end;
        if let Some(block) = paragraph.blocks.last_mut() {
            block.range.end = range.end;
        }
    }

    fn start_block(&mut self, range: Range<usize>, line: &str) {
        let block = Block {
            range: range.clone(),
            table: (self.table_depth > 0).then(|| self.tables_started - 1),
            caveat: self.table_depth == 0 && is_caveat(line),
        };
        match &mut self.open {
            Some(paragraph) => {
                paragraph.range.end = range.end;
                paragraph.blocks.push(block);
            }
            None => {
                self.open = Some(Paragraph {
                    range,
                    blocks: vec![block],
                });
            }
        }
    }

    fn close_paragraph(&mut self) {
        self.paragraphs.extend(self.open.take());
    }

    fn finish(mut self) -> Vec<Paragraph> {
        self.close_paragraph();
        self.paragraphs
    }
}

/// Whether a line opens a caveat: a note, a "however" or a footnote.
///
/// After list markers, indentation, quote marks and an opening parenthesis, the
/// line starts (ignoring case) with `note:`, `notes:`, the words `note that`
/// or `however`, or a `<ref>` tag. The set is deliberately small: a wrong guess
/// only keeps two lines together, but each addition has to be a construct whose
/// meaning is understood.
fn is_caveat(line: &str) -> bool {
    let lead = line.trim_start_matches(|character: char| {
        character.is_whitespace() || matches!(character, '*' | '#' | ':' | ';' | '\'' | '(')
    });
    ["note:", "notes:"]
        .iter()
        .any(|prefix| starts_with_ignore_case(lead, prefix))
        || starts_with_word(lead, "note that")
        || starts_with_word(lead, "however")
        || starts_with_ref_tag(lead)
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Whether the text opens a `<ref>` tag, not another tag that begins with the
/// same letters such as `<references />`: the name must be followed by
/// whitespace, `>` or `/`.
fn starts_with_ref_tag(text: &str) -> bool {
    starts_with_ignore_case(text, "<ref")
        && text[4..]
            .chars()
            .next()
            .is_some_and(|next| next.is_whitespace() || matches!(next, '>' | '/'))
}

fn starts_with_word(text: &str, word: &str) -> bool {
    starts_with_ignore_case(text, word)
        && !text[word.len()..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(text: &str) -> Vec<&str> {
        paragraphs(text)
            .iter()
            .flat_map(|paragraph| &paragraph.blocks)
            .map(|block| &text[block.range.clone()])
            .collect()
    }

    fn paragraph_texts(text: &str) -> Vec<&str> {
        paragraphs(text)
            .iter()
            .map(|paragraph| &text[paragraph.range.clone()])
            .collect()
    }

    #[test]
    fn every_plain_line_is_a_block_and_blank_lines_separate_paragraphs() {
        let text = "one\ntwo\n\n  \nthree\r\nfour";
        assert_eq!(blocks(text), ["one", "two", "three", "four"]);
        assert_eq!(paragraph_texts(text), ["one\ntwo", "three\r\nfour"]);
    }

    #[test]
    fn empty_and_blank_text_has_no_paragraphs() {
        assert_eq!(paragraphs(""), []);
        assert_eq!(paragraphs(" \n\t\n\n"), []);
    }

    #[test]
    fn list_items_are_separate_blocks() {
        assert_eq!(
            blocks("* one\n* two\n** nested\n# three"),
            ["* one", "* two", "** nested", "# three"]
        );
    }

    #[test]
    fn a_multiline_template_call_is_one_block() {
        let text = "before\n{{Box\n|a=1\n\n|b={{Inner|x}}\n}}\nafter";
        assert_eq!(
            blocks(text),
            ["before", "{{Box\n|a=1\n\n|b={{Inner|x}}\n}}", "after"]
        );
        assert_eq!(paragraphs(text).len(), 1);
    }

    #[test]
    fn a_multiline_comment_or_literal_tag_is_one_block() {
        assert_eq!(
            blocks("<!-- a\n\nb -->\nx\n<pre>\nraw\n\nraw\n</pre>\ny"),
            ["<!-- a\n\nb -->", "x", "<pre>\nraw\n\nraw\n</pre>", "y"]
        );
    }

    #[test]
    fn a_line_continues_markup_only_inside_an_open_construct() {
        let text = "{{Box\n|a=1\n}}\nplain";
        let block = &paragraphs(text)[0].blocks[0];
        let flags: Vec<bool> = lines(text, block)
            .iter()
            .map(|line| line.continues_markup)
            .collect();
        assert_eq!(flags, [false, true, true]);
        let ranges: Vec<&str> = lines(text, block)
            .iter()
            .map(|line| &text[line.range.clone()])
            .collect();
        assert_eq!(ranges, ["{{Box", "|a=1", "}}"]);
    }

    #[test]
    fn table_rows_are_blocks_and_the_table_is_numbered() {
        let text = "{| class=x\n! Head\n|-\n| a\n| b\n|-\n| c\n|}\n\ntext\n{|\n|-\n| d\n|}";
        let found = paragraphs(text);
        assert_eq!(found.len(), 2);
        let rows: Vec<(&str, Option<usize>)> = found
            .iter()
            .flat_map(|paragraph| &paragraph.blocks)
            .map(|block| (&text[block.range.clone()], block.table))
            .collect();
        assert_eq!(
            rows,
            [
                ("{| class=x\n! Head", Some(0)),
                ("|-\n| a\n| b", Some(0)),
                ("|-\n| c\n|}", Some(0)),
                ("text", None),
                ("{|", Some(1)),
                ("|-\n| d\n|}", Some(1)),
            ]
        );
    }

    #[test]
    fn a_blank_line_inside_a_table_does_not_end_the_paragraph() {
        let text = "{|\n|-\n| a\n\n| b\n|-\n| c\n|}";
        assert_eq!(paragraphs(text).len(), 1);
        assert_eq!(blocks(text), ["{|", "|-\n| a\n\n| b", "|-\n| c\n|}"]);
    }

    #[test]
    fn a_nested_table_stays_inside_its_row() {
        let text = "{|\n|-\n|\n{|\n|-\n| inner\n|}\n|-\n| next\n|}";
        assert_eq!(
            blocks(text),
            ["{|", "|-\n|\n{|\n|-\n| inner\n|}", "|-\n| next\n|}"]
        );
    }

    #[test]
    fn an_unclosed_table_runs_to_the_end_and_then_text_is_still_table() {
        let text = "{|\n|-\n| a\n\nstill table\n|-\n| b";
        assert_eq!(blocks(text), ["{|", "|-\n| a\n\nstill table", "|-\n| b"]);
        assert_eq!(paragraphs(text).len(), 1);
    }

    #[test]
    fn a_stray_table_closer_does_not_open_anything() {
        assert_eq!(blocks("|}\n|-\ntext"), ["|}", "|-", "text"]);
    }

    #[test]
    fn caveats_are_recognised_by_their_opening_words_only() {
        for caveat in [
            "Note: the drop rate is lower",
            "'''Note:''' bold",
            "* note: in a list",
            ": Notes: indented",
            "Note that it fails",
            "However, the rate",
            "(however it works",
            "<ref name=a>Footnote</ref>",
            "<REF>x</ref>",
            "<ref>x</ref>",
            "<ref/>",
            "<ref name=a />",
        ] {
            assert!(is_caveat(caveat), "{caveat}");
        }
        for plain in [
            "Noted for speed",
            "Notes on speed",
            "Howeverly",
            "Note thatched roofs",
            "<references />",
            "<references/>",
            "<refs>x</refs>",
            "<ref",
            "A note: in the middle",
            "Whoever",
            "",
            "{{Note|text}}",
            "é",
        ] {
            assert!(!is_caveat(plain), "{plain}");
        }
    }

    #[test]
    fn caveat_blocks_are_flagged_but_table_rows_never_are() {
        let found = paragraphs("claim\nNote: x\n{|\n|-\n| Note: y\n|}");
        let flags: Vec<bool> = found[0].blocks.iter().map(|block| block.caveat).collect();
        assert_eq!(flags, [false, true, false, false]);
    }

    #[test]
    fn non_ascii_text_is_handled_on_character_boundaries() {
        assert_eq!(blocks("é☕\n\n🐟{{é\n☕}}"), ["é☕", "🐟{{é\n☕}}"]);
        assert!(!is_caveat("éé"));
    }

    #[test]
    fn blocks_cover_every_non_blank_line_in_order() {
        let text = "a\n{{T\n|x\n}}\n\n{|\n|-\n| r\n|}\nb\n\n\nc";
        let joined: String = blocks(text).concat();
        let expected: String = text
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let joined: String = joined
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        assert_eq!(joined, expected);
    }
}
