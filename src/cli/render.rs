//! Plain-text output of the Wiki commands. Every function is pure: it returns
//! the text to print and leaves printing and exit codes to the caller.
//!
//! Wiki content is untrusted, and a terminal acts on control characters, so
//! everything taken from an export is passed through [`plain`] first.

use std::fmt::Write as _;

use crate::wiki::commands::{ImportReport, PageReport, SearchReport};
use crate::wiki::{ChunkKind, Loss, Resolution, SearchHit, StoredPage};

/// How many characters of a chunk a search hit shows.
const SNIPPET_CHARS: usize = 240;

/// Text for the `wiki import` report.
#[must_use]
pub fn import(report: &ImportReport) -> String {
    let mut text = format!("Imported pages from {}\n", plain(report.source.site_name()));
    let export = &report.export;
    let _ = writeln!(
        text,
        "In the export: {} pages, {} of them redirects",
        export.pages_imported, export.redirects_imported
    );
    let stored = &report.stored;
    let _ = writeln!(
        text,
        "Stored: {} added, {} replaced, {} unchanged",
        stored.added, stored.replaced, stored.unchanged
    );
    if export.total_pages_skipped() > 0 {
        let _ = writeln!(text, "Skipped as invalid: {}", export.total_pages_skipped());
        for (kind, count) in &export.pages_skipped {
            let _ = writeln!(text, "  {kind}: {count}");
        }
    }
    text
}

/// Text for the `wiki search` report.
#[must_use]
pub fn search(report: &SearchReport) -> String {
    if report.hits.is_empty() {
        return "No matches.\n".to_owned();
    }
    let mut text = String::new();
    for (index, hit) in report.hits.iter().enumerate() {
        let _ = write!(text, "{}. ", index + 1);
        write_hit(&mut text, hit);
    }
    text
}

fn write_hit(text: &mut String, hit: &SearchHit) {
    let mut place = plain(hit.title.as_str());
    for heading in &hit.chunk.section_path {
        let _ = write!(place, " > {}", plain(heading));
    }
    let _ = writeln!(
        text,
        "{place}\n   page {}, revision {}, {}{}",
        hit.page.get(),
        hit.revision.get(),
        hit.timestamp.as_str(),
        markers(hit),
    );
    let _ = writeln!(text, "   {}", snippet(&hit.chunk.text));
}

/// Notes that change how far a hit can be trusted to be the whole story.
fn markers(hit: &SearchHit) -> String {
    let mut notes = Vec::new();
    if hit.chunk.kind == ChunkKind::TemplateOnly {
        notes.push("only unexpanded templates".to_owned());
    }
    if !hit.chunk.losses.is_empty() {
        let kinds: Vec<&str> = hit
            .chunk
            .losses
            .iter()
            .map(|loss| loss_name(*loss))
            .collect();
        notes.push(format!("cut: {}", kinds.join(", ")));
    }
    if notes.is_empty() {
        String::new()
    } else {
        format!(" [{}]", notes.join("; "))
    }
}

const fn loss_name(loss: Loss) -> &'static str {
    match loss {
        Loss::CutLine => "line",
        Loss::SplitMarkup => "markup",
        Loss::SplitTable => "table",
        Loss::SplitCaveat => "caveat",
    }
}

/// Text for the `wiki page` report: the page on success, or a message saying
/// why the title did not lead to one.
///
/// # Errors
///
/// The `Err` text is the message for a title that is missing, loops or is
/// ambiguous; the caller reports it as a failure.
pub fn page(report: &PageReport) -> Result<String, String> {
    let via: Vec<String> = report
        .redirects
        .iter()
        .map(|title| plain(title.as_str()))
        .collect();
    let followed = if via.is_empty() {
        String::new()
    } else {
        format!(" (redirected from {})", via.join(" -> "))
    };
    match &report.resolution {
        Resolution::Found { page, .. } => Ok(page_text(page, &followed)),
        Resolution::Absent => Err("No page has that title.".to_owned()),
        Resolution::MissingTarget { target, .. } => Err(format!(
            "The redirect{followed} points at \"{}\", which is not in the database.",
            plain(target.as_str())
        )),
        Resolution::Loop { .. } => Err(format!(
            "The redirects{followed} lead back to a page already followed."
        )),
        Resolution::TooManyRedirects { .. } => Err(format!(
            "The redirects{followed} are longer than the chain this tool follows."
        )),
        Resolution::Ambiguous { pages, .. } => {
            let ids: Vec<String> = pages.iter().map(|id| id.get().to_string()).collect();
            Err(format!(
                "Several stored pages have that title (page ids {}); the database cannot tell which is current.",
                ids.join(", ")
            ))
        }
    }
}

fn page_text(page: &StoredPage, followed: &str) -> String {
    format!(
        "{title}{followed}\npage {id}, revision {revision}, {time}\n\n{body}\n",
        title = plain(page.title.as_str()),
        id = page.id.get(),
        revision = page.revision.get(),
        time = page.timestamp.as_str(),
        body = plain(&page.text),
    )
}

/// The start of a chunk on one line.
fn snippet(text: &str) -> String {
    let collapsed = plain(text).split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = collapsed.chars();
    let head: String = characters.by_ref().take(SNIPPET_CHARS).collect();
    if characters.next().is_some() {
        format!("{head}...")
    } else {
        head
    }
}

/// `text` with control characters other than line breaks and tabs replaced, so
/// that content from an export cannot move the cursor, change colours or set a
/// title in the terminal it is printed to.
fn plain(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\t') {
                '\u{FFFD}'
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::wiki::{PageId, PageTitle, RevisionId, RevisionTimestamp, SourceId, StoredChunk};

    use super::*;

    fn title(text: &str) -> PageTitle {
        PageTitle::new(text.to_owned()).unwrap()
    }

    fn source() -> SourceId {
        SourceId::new(
            "Example Wiki".to_owned(),
            None,
            "https://wiki.example/w/Main_Page".to_owned(),
        )
        .unwrap()
    }

    fn stored_page(text: &str) -> StoredPage {
        StoredPage {
            id: PageId::new(7).unwrap(),
            namespace: 0,
            title: title("Axe"),
            revision: RevisionId::new(70).unwrap(),
            timestamp: RevisionTimestamp::parse("2026-03-04T05:06:07Z").unwrap(),
            hash: None,
            redirect_target: None,
            text: text.to_owned(),
        }
    }

    fn hit(kind: ChunkKind, losses: Vec<Loss>, text: &str) -> SearchHit {
        SearchHit {
            page: PageId::new(7).unwrap(),
            title: title("Axe"),
            revision: RevisionId::new(70).unwrap(),
            timestamp: RevisionTimestamp::parse("2026-03-04T05:06:07Z").unwrap(),
            chunk: StoredChunk {
                section: 1,
                section_path: vec!["Uses".to_owned(), "Trees".to_owned()],
                section_occurrence: 1,
                section_level: 3,
                index: 0,
                source: 0..text.len(),
                kind,
                losses,
                templates: Vec::new(),
                link_targets: Vec::new(),
                text: text.to_owned(),
            },
        }
    }

    fn page_report(resolution: Resolution, redirects: &[&str]) -> PageReport {
        PageReport {
            source: source(),
            resolution,
            redirects: redirects.iter().map(|text| title(text)).collect(),
        }
    }

    #[test]
    fn a_search_lists_each_hit_with_its_citation_and_section_path() {
        let report = SearchReport {
            source: source(),
            hits: vec![hit(ChunkKind::Prose, Vec::new(), "Chops\ntrees.")],
        };
        assert_eq!(
            search(&report),
            "1. Axe > Uses > Trees\n   page 7, revision 70, 2026-03-04T05:06:07Z\n   Chops trees.\n"
        );
    }

    #[test]
    fn a_hit_says_when_its_chunk_is_only_templates_or_was_cut() {
        let report = SearchReport {
            source: source(),
            hits: vec![hit(
                ChunkKind::TemplateOnly,
                vec![Loss::SplitTable, Loss::SplitCaveat],
                "{{Infobox}}",
            )],
        };
        let text = search(&report);
        assert!(
            text.contains("[only unexpanded templates; cut: table, caveat]"),
            "{text}"
        );
    }

    #[test]
    fn a_search_without_hits_says_so() {
        let report = SearchReport {
            source: source(),
            hits: Vec::new(),
        };
        assert_eq!(search(&report), "No matches.\n");
    }

    #[test]
    fn a_found_page_shows_its_provenance_and_the_redirects_followed() {
        let report = page_report(
            Resolution::Found {
                page: stored_page("Body text."),
                redirects: vec![PageId::new(2).unwrap()],
            },
            &["Hatchet"],
        );
        assert_eq!(
            page(&report).unwrap(),
            "Axe (redirected from Hatchet)\npage 7, revision 70, 2026-03-04T05:06:07Z\n\nBody text.\n"
        );
    }

    #[test]
    fn each_failed_resolution_has_its_own_message() {
        let id = |value| PageId::new(value).unwrap();
        let message = |resolution, via: &[&str]| page(&page_report(resolution, via)).unwrap_err();
        assert_eq!(message(Resolution::Absent, &[]), "No page has that title.");
        let missing = message(
            Resolution::MissingTarget {
                target: title("Gone page"),
                redirects: vec![id(2)],
            },
            &["Hatchet"],
        );
        assert!(
            missing.contains("\"Gone page\", which is not in the database"),
            "{missing}"
        );
        assert!(
            message(
                Resolution::Loop {
                    redirects: vec![id(1)]
                },
                &["A"]
            )
            .contains("lead back")
        );
        assert!(
            message(
                Resolution::TooManyRedirects {
                    redirects: vec![id(1)]
                },
                &["A"]
            )
            .contains("longer than")
        );
        let ambiguous = message(
            Resolution::Ambiguous {
                pages: vec![id(3), id(9)],
                redirects: Vec::new(),
            },
            &[],
        );
        assert!(ambiguous.contains("page ids 3, 9"), "{ambiguous}");
    }

    #[test]
    fn page_text_is_cleaned_of_control_characters() {
        let report = page_report(
            Resolution::Found {
                page: stored_page("safe\u{1b}[2Jtext"),
                redirects: Vec::new(),
            },
            &[],
        );
        let text = page(&report).unwrap();
        assert!(!text.contains('\u{1b}'), "{text:?}");
    }

    #[test]
    fn control_characters_are_replaced_but_text_layout_is_kept() {
        assert_eq!(
            plain("a\u{1b}[31mb\u{7}\tc\nd\u{0}"),
            "a\u{FFFD}[31mb\u{FFFD}\tc\nd\u{FFFD}"
        );
        assert_eq!(plain("é☕ plain"), "é☕ plain");
    }

    #[test]
    fn a_snippet_is_one_line_and_bounded() {
        assert_eq!(snippet("one\n\n  two\tthree"), "one two three");
        let long = "x".repeat(SNIPPET_CHARS + 10);
        let shown = snippet(&long);
        assert_eq!(shown.chars().count(), SNIPPET_CHARS + 3);
        assert!(shown.ends_with("..."));
        assert_eq!(snippet(&"y".repeat(SNIPPET_CHARS)).len(), SNIPPET_CHARS);
    }

    #[test]
    fn a_snippet_cuts_on_character_boundaries() {
        let text = "é".repeat(SNIPPET_CHARS + 5);
        assert!(snippet(&text).starts_with('é'));
    }
}
