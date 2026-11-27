//! Finds section headings in wikitext without interpreting anything else.
//!
//! A heading is a line that starts and ends with `=` signs. Lines inside
//! comments, raw-text tags and template calls are not headings, so the scanner
//! tracks just enough state to know that. It never expands, renders or executes
//! anything.
//!
//! Known limits, deliberately not worked around because that would need a
//! renderer: a `>` inside a quoted tag attribute ends the tag early, an opening
//! tag split across lines is not recognised, and brace counting treats `{{{`
//! parameters like template calls.

use std::ops::Range;

/// Tags whose content is literal text, so headings inside it do not count.
const RAW_TAGS: [&str; 5] = ["nowiki", "pre", "syntaxhighlight", "source", "math"];
const MAX_HEADING_LEVEL: usize = 6;

/// A heading line and where its section's body begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Heading {
    /// 1 to 6, from the shorter side of the surrounding `=` signs.
    pub(super) level: u8,
    /// The heading text between the `=` signs, trimmed. Markup is kept raw.
    pub(super) title: String,
    /// Byte range of the heading line, without its line ending.
    pub(super) line: Range<usize>,
    /// Byte offset of the first byte after the heading line and its ending.
    pub(super) body_start: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    Plain,
    Comment,
    /// Inside the raw-text tag at this index of [`RAW_TAGS`].
    Raw(usize),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ScanState {
    region: Region,
    /// How many `{{` are open. Template arguments may span lines.
    template_depth: usize,
}

impl ScanState {
    /// The state at the start of a text: no comment, literal tag or template
    /// is open.
    pub(super) const fn new() -> Self {
        Self {
            region: Region::Plain,
            template_depth: 0,
        }
    }

    /// Whether the next byte is outside every comment, literal tag and
    /// template call.
    pub(super) fn is_top_level(&self) -> bool {
        self.region == Region::Plain && self.template_depth == 0
    }
}

/// Longest heading title kept, in bytes. Real headings are a few dozen bytes;
/// the bound stops every section path from carrying an enormous title.
pub(super) const MAX_HEADING_BYTES: usize = 512;

/// Why headings could not be collected within the limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScanError {
    /// There are more headings than the caller allows.
    TooManyHeadings,
    /// A heading title is longer than [`MAX_HEADING_BYTES`].
    HeadingTooLong,
}

/// Returns the headings of `text` in document order.
///
/// # Errors
///
/// Returns [`ScanError::TooManyHeadings`] if there are more than `max_headings`
/// and [`ScanError::HeadingTooLong`] if a title exceeds [`MAX_HEADING_BYTES`],
/// without scanning further.
pub(super) fn find_headings(text: &str, max_headings: usize) -> Result<Vec<Heading>, ScanError> {
    let mut headings = Vec::new();
    let mut state = ScanState::new();
    let mut offset = 0;
    for raw_line in text.split_inclusive('\n') {
        let line_end = offset + raw_line.len();
        let line = raw_line.trim_end_matches('\n').trim_end_matches('\r');
        if let Some((level, title)) = parse_heading(line).filter(|_| state.is_top_level()) {
            if headings.len() >= max_headings {
                return Err(ScanError::TooManyHeadings);
            }
            if title.len() > MAX_HEADING_BYTES {
                return Err(ScanError::HeadingTooLong);
            }
            headings.push(Heading {
                level,
                title,
                line: offset..offset + line.len(),
                body_start: line_end,
            });
        }
        advance(&mut state, line);
        offset = line_end;
    }
    Ok(headings)
}

/// Parses `== Title ==`. The level is the shorter run of `=`; extra `=` on the
/// longer side stay in the title, as `MediaWiki` does.
fn parse_heading(line: &str) -> Option<(u8, String)> {
    let line = strip_trailing_comments(line.trim_end());
    let opening = line.bytes().take_while(|&byte| byte == b'=').count();
    let closing = line.bytes().rev().take_while(|&byte| byte == b'=').count();
    if opening == 0 || closing == 0 || opening + closing >= line.len() {
        return None;
    }
    let level = opening.min(closing).min(MAX_HEADING_LEVEL);
    let title = line[level..line.len() - level].trim();
    if title.is_empty() {
        return None;
    }
    Some((u8::try_from(level).ok()?, title.to_owned()))
}

/// Drops complete `<!-- ... -->` comments that end the line, which may follow a
/// heading.
fn strip_trailing_comments(mut line: &str) -> &str {
    while let Some(without_end) = line.strip_suffix("-->") {
        let Some(start) = without_end.rfind("<!--") else {
            break;
        };
        line = line[..start].trim_end();
    }
    line
}

/// Updates the state for the text of one line.
pub(super) fn advance(state: &mut ScanState, line: &str) {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &bytes[index..];
        match state.region {
            Region::Comment => match find(rest, b"-->") {
                Some(end) => {
                    state.region = Region::Plain;
                    index += end + 3;
                }
                None => return,
            },
            Region::Raw(tag) => match find_closing_tag(rest, RAW_TAGS[tag]) {
                Some(end) => {
                    state.region = Region::Plain;
                    index += end;
                }
                None => return,
            },
            Region::Plain => index += advance_plain(state, rest),
        }
    }
}

/// Consumes one construct in plain text and returns how many bytes it spanned.
fn advance_plain(state: &mut ScanState, rest: &[u8]) -> usize {
    if rest.starts_with(b"<!--") {
        state.region = Region::Comment;
        4
    } else if rest.starts_with(b"{{") {
        state.template_depth += 1;
        2
    } else if rest.starts_with(b"}}") {
        state.template_depth = state.template_depth.saturating_sub(1);
        2
    } else if rest.starts_with(b"<") {
        advance_tag(state, rest)
    } else {
        1
    }
}

/// A comment or literal-tag element, which is not wikitext markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Literal {
    /// `<!-- ... -->`, which renders as nothing.
    Comment(usize),
    /// A literal-tag element such as `<nowiki>...</nowiki>`, whose content
    /// renders as plain text.
    Raw(usize),
}

impl Literal {
    /// The element's length in bytes.
    pub(super) const fn len(self) -> usize {
        match self {
            Self::Comment(len) | Self::Raw(len) => len,
        }
    }
}

/// The comment or literal-tag element at the start of `rest`, which may span
/// lines. An element that is never closed runs to the end of `rest`.
pub(super) fn literal_at(rest: &[u8]) -> Option<Literal> {
    if let Some(after_opening) = rest.strip_prefix(b"<!--") {
        let end = find(after_opening, b"-->").map_or(after_opening.len(), |at| at + 3);
        return Some(Literal::Comment(4 + end));
    }
    let (tag, _) = raw_tag_at(rest)?;
    let Some(close) = rest.iter().position(|&byte| byte == b'>') else {
        return Some(Literal::Raw(rest.len()));
    };
    let opening_end = close + 1;
    if rest[..close].ends_with(b"/") {
        return Some(Literal::Raw(opening_end));
    }
    let end = find_closing_tag(&rest[opening_end..], RAW_TAGS[tag])
        .map_or(rest.len(), |at| opening_end + at);
    Some(Literal::Raw(end))
}

/// Handles `<` in plain text: an opening raw-text tag starts a raw region.
fn advance_tag(state: &mut ScanState, rest: &[u8]) -> usize {
    let Some((tag, name_end)) = raw_tag_at(rest) else {
        return 1;
    };
    let Some(close) = rest.iter().position(|&byte| byte == b'>') else {
        return rest.len();
    };
    let self_closing = rest[..close].ends_with(b"/");
    if !self_closing {
        state.region = Region::Raw(tag);
    }
    close.max(name_end) + 1
}

/// The raw-text tag opened at the start of `rest`, with the length of its name
/// including the `<`.
fn raw_tag_at(rest: &[u8]) -> Option<(usize, usize)> {
    RAW_TAGS.iter().enumerate().find_map(|(index, name)| {
        let end = name.len() + 1;
        let matches_name = rest.get(1..end)?.eq_ignore_ascii_case(name.as_bytes());
        let delimited = matches!(rest.get(end), Some(b'>' | b' ' | b'/' | b'\t'));
        (matches_name && delimited).then_some((index, end))
    })
}

/// The offset just after the first `</name ...>` in `rest`, or the end of
/// `rest` if the closing tag has no `>` on this line.
fn find_closing_tag(rest: &[u8], name: &str) -> Option<usize> {
    let mut index = 0;
    while let Some(found) = find(&rest[index..], b"</") {
        let start = index + found;
        let name_end = start + 2 + name.len();
        let names_the_tag = rest
            .get(start + 2..name_end)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name.as_bytes()));
        // `</nowikix>` is another tag, so a delimiter must follow the name.
        let delimited = matches!(rest.get(name_end), None | Some(b'>' | b' ' | b'\t'));
        if names_the_tag && delimited {
            return Some(
                rest[start..]
                    .iter()
                    .position(|&byte| byte == b'>')
                    .map_or(rest.len(), |close| start + close + 1),
            );
        }
        index = start + 2;
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headings(text: &str) -> Vec<(u8, String)> {
        find_headings(text, 1000)
            .unwrap()
            .into_iter()
            .map(|heading| (heading.level, heading.title))
            .collect()
    }

    fn titled(level: u8, title: &str) -> (u8, String) {
        (level, title.to_owned())
    }

    #[test]
    fn finds_headings_of_each_level_in_order() {
        let text = "intro\n= One =\na\n== Two ==\nb\n=== Three ===\n====== Six ======\n";
        assert_eq!(
            headings(text),
            [
                titled(1, "One"),
                titled(2, "Two"),
                titled(3, "Three"),
                titled(6, "Six")
            ]
        );
    }

    #[test]
    fn records_where_each_heading_line_and_body_are() {
        let text = "lead\n== A ==\nbody\n";
        let found = find_headings(text, 10).unwrap();
        assert_eq!(&text[found[0].line.clone()], "== A ==");
        assert_eq!(&text[found[0].body_start..], "body\n");
    }

    #[test]
    fn the_shorter_side_sets_the_level_and_extra_signs_stay_in_the_title() {
        assert_eq!(headings("=== A ==\n"), [titled(2, "= A")]);
        assert_eq!(headings("== A ====\n"), [titled(2, "A ==")]);
        assert_eq!(headings("======= Deep =======\n"), [titled(6, "= Deep =")]);
    }

    #[test]
    fn whitespace_and_trailing_comments_around_a_heading_are_allowed() {
        assert_eq!(headings("==A==  \n"), [titled(2, "A")]);
        assert_eq!(headings("== A == <!-- note --> \n"), [titled(2, "A")]);
        assert_eq!(headings("== A ==\r\nbody\r\n"), [titled(2, "A")]);
    }

    #[test]
    fn lines_that_only_look_like_headings_are_not_headings() {
        for text in [
            "====\n",
            "==\n",
            "== A\n",
            "A ==\n",
            "= =\n",
            " == A ==\n",
            "text == A ==\n",
            "==  ==\n",
        ] {
            assert_eq!(headings(text), [], "{text:?}");
        }
    }

    #[test]
    fn headings_inside_comments_are_ignored() {
        assert_eq!(
            headings("<!--\n== Hidden ==\n-->\n== Shown ==\n"),
            [titled(2, "Shown")]
        );
        assert_eq!(
            headings("a <!-- x --> b\n== Shown ==\n"),
            [titled(2, "Shown")]
        );
    }

    #[test]
    fn headings_inside_literal_tags_are_ignored() {
        for text in [
            "<nowiki>\n== Hidden ==\n</nowiki>\n== Shown ==\n",
            "<PRE>\n== Hidden ==\n</pre>\n== Shown ==\n",
            "<syntaxhighlight lang=\"x\">\n== Hidden ==\n</syntaxhighlight>\n== Shown ==\n",
            "<math>\n== Hidden ==\n</math >\n== Shown ==\n",
        ] {
            assert_eq!(headings(text), [titled(2, "Shown")], "{text:?}");
        }
    }

    #[test]
    fn a_closing_tag_must_match_the_whole_tag_name() {
        let text = "<nowiki>\n</nowikix>\n== Still hidden ==\n</nowiki>\n== Shown ==\n";
        assert_eq!(headings(text), [titled(2, "Shown")]);
        let spaced = "<pre>\n</pre >\n== Shown ==\n";
        assert_eq!(headings(spaced), [titled(2, "Shown")]);
    }

    #[test]
    fn a_heading_title_longer_than_the_bound_is_an_error() {
        let at_limit = format!("== {} ==\n", "a".repeat(MAX_HEADING_BYTES));
        let over = format!("== {} ==\n", "a".repeat(MAX_HEADING_BYTES + 1));
        assert!(find_headings(&at_limit, 10).is_ok());
        assert_eq!(find_headings(&over, 10), Err(ScanError::HeadingTooLong));
    }

    #[test]
    fn a_self_closing_literal_tag_does_not_open_a_region() {
        assert_eq!(headings("<nowiki />\n== Shown ==\n"), [titled(2, "Shown")]);
    }

    #[test]
    fn tags_that_merely_start_like_a_literal_tag_do_not_open_one() {
        assert_eq!(headings("<prefix>\n== Shown ==\n"), [titled(2, "Shown")]);
        assert_eq!(headings("<sourcery>\n== Shown ==\n"), [titled(2, "Shown")]);
    }

    #[test]
    fn headings_inside_multiline_templates_are_ignored() {
        let text =
            "{{Box\n|text=\n== Hidden ==\n|other={{Inner|x}}\n== Also hidden ==\n}}\n== Shown ==\n";
        assert_eq!(headings(text), [titled(2, "Shown")]);
    }

    #[test]
    fn unbalanced_braces_cannot_hide_the_rest_forever() {
        // A stray `}}` must not make depth negative and swallow later braces.
        assert_eq!(headings("}}\n{{A}}\n== Shown ==\n"), [titled(2, "Shown")]);
    }

    #[test]
    fn an_unclosed_comment_or_literal_tag_runs_to_the_end_without_panicking() {
        assert_eq!(headings("<!-- never closed\n== Hidden ==\n"), []);
        assert_eq!(headings("<nowiki>\n== Hidden ==\n"), []);
        assert_eq!(headings("<nowiki"), []);
    }

    #[test]
    fn non_ascii_text_is_handled_on_character_boundaries() {
        assert_eq!(headings("é<é{{é\n== Äpfel ☕ ==\n"), []);
        assert_eq!(headings("é<é\n== Äpfel ☕ ==\n"), [titled(2, "Äpfel ☕")]);
    }

    #[test]
    fn too_many_headings_is_an_error_instead_of_unbounded_growth() {
        let text = "= a =\n= b =\n= c =\n";
        assert_eq!(find_headings(text, 3).map(|found| found.len()), Ok(3));
        assert_eq!(find_headings(text, 2), Err(ScanError::TooManyHeadings));
    }
}
