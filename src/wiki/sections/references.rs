//! Lists what a chunk of wikitext refers to but does not contain: the
//! templates it calls and the pages it links to.
//!
//! Nothing here is resolved. A template call stands for text this crate never
//! produces, so its name is reported rather than its output, and a link target
//! is only a name that may or may not exist in the corpus. Whether a chunk says
//! anything beyond template calls is reported too, so that a reader can tell a
//! chunk of prose from a chunk that is mostly an unexpanded infobox.
//!
//! The scan is one pass over the text with the same comment, literal-tag and
//! brace rules as the heading scanner, including its known limits, and never
//! allocates more than the names it reports, which are pieces of the text.

use std::collections::HashSet;

use super::scan::{self, Literal};

/// What one chunk refers to, each name once, in order of first appearance.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct References {
    /// Names of the templates and parser functions called, as written and
    /// trimmed. A parser function such as `{{#if:...}}` is reported as `#if`.
    /// `{{{parameter}}}` is not a call.
    pub(super) templates: Vec<String>,
    /// Targets of `[[...]]` links, as written: without a leading colon, `|label`
    /// or `#section`. Includes file and category links; the caller decides
    /// which namespaces matter.
    pub(super) link_targets: Vec<String>,
    /// Whether any text outside template calls and comments is not whitespace.
    pub(super) has_prose: bool,
}

/// Bytes that end a template name or a link target: none can appear in a page
/// title, so the first one marks the end of the name. Stopping at `{` and `[`
/// also keeps the scan linear in the length of the text.
const NAME_END: [u8; 8] = *b"|{}[]<>\n";

pub(super) fn scan(text: &str) -> References {
    let bytes = text.as_bytes();
    let mut found = References::default();
    let mut seen_templates = HashSet::new();
    let mut seen_links = HashSet::new();
    let mut depth = 0usize;
    let mut index = 0;
    while index < bytes.len() {
        let rest = &bytes[index..];
        if let Some(literal) = scan::literal_at(rest) {
            found.has_prose |= matches!(literal, Literal::Raw(_)) && depth == 0;
            index += literal.len();
        } else if rest.starts_with(b"{{") {
            depth += 1;
            if let Some(name) = template_name(&text[index + 2..])
                && seen_templates.insert(name)
            {
                found.templates.push(name.to_owned());
            }
            index += 2;
        } else if rest.starts_with(b"}}") {
            depth = depth.saturating_sub(1);
            index += 2;
        } else if rest.starts_with(b"[[") {
            found.has_prose |= depth == 0;
            if let Some(target) = link_target(&text[index + 2..])
                && seen_links.insert(target)
            {
                found.link_targets.push(target.to_owned());
            }
            index += 2;
        } else {
            found.has_prose |= depth == 0 && !rest[0].is_ascii_whitespace();
            index += 1;
        }
    }
    found
}

/// The name after `{{`, up to the first byte that cannot be part of one.
/// A parser function ends at its `:`. `None` if the name is empty.
fn template_name(after_braces: &str) -> Option<&str> {
    let end = name_end(after_braces);
    let mut name = after_braces[..end].trim();
    if name.starts_with('#') {
        name = name.split(':').next().unwrap_or(name).trim_end();
    }
    (!name.is_empty()).then_some(name)
}

/// The page named by a link, up to its `|label` or closing brackets. `None`
/// for an unterminated link, an empty or dynamic target and a pure `#section`
/// link.
fn link_target(after_brackets: &str) -> Option<&str> {
    let end = name_end(after_brackets);
    let terminated = matches!(after_brackets.as_bytes().get(end), Some(b'|'))
        || after_brackets[end..].starts_with("]]");
    if !terminated {
        return None;
    }
    let target = after_brackets[..end].trim().trim_start_matches(':');
    let page = target.split('#').next().unwrap_or(target).trim();
    (!page.is_empty()).then_some(page)
}

fn name_end(text: &str) -> usize {
    text.bytes()
        .position(|byte| NAME_END.contains(&byte))
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn templates(text: &str) -> Vec<String> {
        scan(text).templates
    }

    fn links(text: &str) -> Vec<String> {
        scan(text).link_targets
    }

    #[test]
    fn template_names_are_reported_as_written_without_arguments() {
        assert_eq!(
            templates("{{Infobox Item|name=Axe}} text {{ Drop table | x }} {{Plain}}"),
            ["Infobox Item", "Drop table", "Plain"]
        );
    }

    #[test]
    fn nested_and_multiline_templates_are_all_listed_once_in_order() {
        let text = "{{Outer\n|a={{Inner|x}}\n|b={{Inner|y}}\n}}\n{{Outer}}";
        assert_eq!(templates(text), ["Outer", "Inner"]);
    }

    #[test]
    fn parser_functions_are_named_without_their_first_argument() {
        assert_eq!(
            templates("{{#if: yes | a | b}} {{#switch:x|y=z}}"),
            ["#if", "#switch"]
        );
    }

    #[test]
    fn parameters_and_empty_calls_are_not_templates() {
        assert_eq!(
            templates("{{{1}}} {{{name|default}}} {{}} {{|x}}"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn templates_inside_comments_and_literal_tags_are_not_calls() {
        assert_eq!(
            templates("<!-- {{A}} --> <nowiki>{{B}}</nowiki> <pre>{{C}}</pre> {{D}}"),
            ["D"]
        );
    }

    #[test]
    fn link_targets_drop_labels_sections_and_leading_colons() {
        assert_eq!(
            links("[[Axe]] [[Pick|a label]] [[:Category:Tools]] [[Bank#Notes|n]] [[Axe]]"),
            ["Axe", "Pick", "Category:Tools", "Bank"]
        );
    }

    #[test]
    fn unusable_links_are_skipped() {
        assert_eq!(
            links("[[#Local]] [[]] [[{{PAGENAME}}]] [[unterminated [[Real]] [[A]x"),
            ["Real"]
        );
    }

    #[test]
    fn links_inside_template_arguments_are_listed() {
        assert_eq!(
            links("{{Box|image=[[File:Axe.png|x]]|see=[[Axe]]}}"),
            ["File:Axe.png", "Axe"]
        );
    }

    #[test]
    fn a_chunk_of_only_template_calls_and_comments_has_no_prose() {
        for text in [
            "{{A}}",
            "{{A|x=text\nmore}}\n<!-- c -->\n{{B}}",
            "  {{A}}  ",
        ] {
            assert!(!scan(text).has_prose, "{text:?}");
        }
        assert!(!scan("").has_prose);
        assert!(!scan("<!-- only -->").has_prose);
    }

    #[test]
    fn text_links_and_literals_outside_templates_are_prose() {
        for text in [
            "word {{A}}",
            "{{A}} [[Link]]",
            "{{A}} <nowiki>x</nowiki>",
            "é",
        ] {
            assert!(scan(text).has_prose, "{text:?}");
        }
    }

    #[test]
    fn a_chunk_that_starts_inside_a_template_reads_as_prose() {
        // The closing braces of a call opened in an earlier chunk meet depth
        // zero, so the text before them reads as prose. Over-counting prose is
        // the safe side: a chunk is never called template-only wrongly.
        assert!(scan("|b=2\n}}").has_prose);
    }

    #[test]
    fn unbalanced_and_hostile_text_neither_panics_nor_loops() {
        for text in [
            "{{{{{{{{",
            "}}}}}}",
            "[[[[[[",
            "{{A|[[B",
            "<nowiki",
            "<!--",
            "é{{é[[é",
            "\u{0}{{\u{0}}}",
        ] {
            let _ = scan(text);
        }
        assert_eq!(templates("{{A}}}}}}{{B}}"), ["A", "B"]);
    }

    #[test]
    fn names_borrowed_from_non_ascii_text_stay_on_character_boundaries() {
        assert_eq!(templates("{{Épée|x}} [[Café]]"), ["Épée"]);
        assert_eq!(links("{{Épée|x}} [[Café]]"), ["Café"]);
    }

    #[test]
    fn scanning_is_deterministic_and_linear_enough_for_a_large_hostile_text() {
        let text = "{{[[".repeat(50_000);
        assert_eq!(scan(&text), scan(&text));
    }
}
