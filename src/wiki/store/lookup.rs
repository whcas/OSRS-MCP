//! Exact-title lookup and redirect resolution over the stored pages.
//!
//! Titles are compared as `MediaWiki` compares them, no more: runs of spaces
//! and underscores are one space, a `#fragment` is not part of a name, and the
//! first letter is case-insensitive (`MediaWiki`'s `first-letter` rule, which
//! the default configuration and the exports this crate reads use). The same
//! [`title_key`] is computed for stored titles when a page is written and for
//! requested titles and redirect targets when they are looked up, so every
//! spelling of a title meets every other; the raw title is kept for display
//! and citation. The rest of a title is case-sensitive, so `AXE` does not find
//! `Axe`. Nothing here reads text or expands templates.

use rusqlite::{Connection, params};

use super::{
    Operation, PageId, PageTitle, SourceId, Store, StoreError, StoredPage, find_source, read_page,
    stored_page_id,
};

/// How many redirects one resolution follows before giving up. `MediaWiki`
/// itself follows one; a few more tolerate double redirects in an export
/// without letting a long chain cost unbounded reads.
const MAX_REDIRECTS: usize = 8;

/// The pages a title names in one source.
#[derive(Debug)]
pub enum TitleMatch {
    /// No stored page has the title.
    Absent,
    /// Exactly one page has it, redirect or not.
    Unique(StoredPage),
    /// Several stored pages have the title, in ascending page id order. A wiki
    /// never exports two live pages with one title, so this means the store
    /// holds a page that was since deleted or moved; the store cannot tell
    /// which is current and does not choose.
    Ambiguous(Vec<PageId>),
}

/// The outcome of following a title through its redirects.
///
/// `redirects` lists the redirect pages followed, in order, so a citation can
/// show how the answer was reached. It is empty when the first page found was
/// not a redirect.
#[derive(Debug)]
pub enum Resolution {
    /// The page the title leads to, which is not a redirect.
    Found {
        /// The page at the end of the chain.
        page: StoredPage,
        /// The redirects followed to reach it.
        redirects: Vec<PageId>,
    },
    /// The requested title is not stored. Distinct from [`Self::MissingTarget`]:
    /// the store was never offered the page, so it says nothing about the wiki.
    Absent,
    /// A redirect points at a title the store does not have.
    MissingTarget {
        /// The target the last redirect declared, as exported.
        target: PageTitle,
        /// The redirects followed, ending with the one that declared `target`.
        redirects: Vec<PageId>,
    },
    /// The redirects lead back to a page already followed.
    Loop {
        /// The redirects followed, ending with the last one before the repeat.
        redirects: Vec<PageId>,
    },
    /// The chain is longer than the store follows.
    TooManyRedirects {
        /// The redirects followed before stopping.
        redirects: Vec<PageId>,
    },
    /// A title on the way names several stored pages; see
    /// [`TitleMatch::Ambiguous`].
    Ambiguous {
        /// The pages that share the title.
        pages: Vec<PageId>,
        /// The redirects followed to reach the title.
        redirects: Vec<PageId>,
    },
}

impl Store {
    /// The stored page with exactly this title, without following redirects.
    ///
    /// # Errors
    ///
    /// Fails on a database error or if a stored value is invalid.
    pub fn page_by_title(&self, source: &SourceId, title: &str) -> Result<TitleMatch, StoreError> {
        let Some(source_key) = find_source(&self.connection, source)? else {
            return Ok(TitleMatch::Absent);
        };
        find_by_title(&self.connection, source_key, title)
    }

    /// The page a title leads to, following redirects.
    ///
    /// # Errors
    ///
    /// Fails on a database error or if a stored value is invalid. A missing,
    /// looping or ambiguous title is a [`Resolution`], not an error.
    pub fn resolve(&self, source: &SourceId, title: &str) -> Result<Resolution, StoreError> {
        let Some(source_key) = find_source(&self.connection, source)? else {
            return Ok(Resolution::Absent);
        };
        resolve(&self.connection, source_key, title)
    }
}

fn resolve(
    connection: &Connection,
    source_key: i64,
    requested: &str,
) -> Result<Resolution, StoreError> {
    let mut redirects = Vec::new();
    let mut next = requested.to_owned();
    let mut declared: Option<PageTitle> = None;
    loop {
        let page = match find_by_title(connection, source_key, &next)? {
            TitleMatch::Unique(page) => page,
            TitleMatch::Ambiguous(pages) => return Ok(Resolution::Ambiguous { pages, redirects }),
            TitleMatch::Absent => {
                return Ok(declared.map_or(Resolution::Absent, |target| {
                    Resolution::MissingTarget { target, redirects }
                }));
            }
        };
        let Some(target) = page.redirect_target else {
            return Ok(Resolution::Found { page, redirects });
        };
        if redirects.contains(&page.id) {
            return Ok(Resolution::Loop { redirects });
        }
        if redirects.len() == MAX_REDIRECTS {
            return Ok(Resolution::TooManyRedirects { redirects });
        }
        redirects.push(page.id);
        target.as_str().clone_into(&mut next);
        declared = Some(target);
    }
}

fn find_by_title(
    connection: &Connection,
    source_key: i64,
    title: &str,
) -> Result<TitleMatch, StoreError> {
    let key = title_key(title);
    if key.is_empty() {
        return Ok(TitleMatch::Absent);
    }
    let ids = page_ids_titled(connection, source_key, &key)?;
    match ids.as_slice() {
        [] => Ok(TitleMatch::Absent),
        [only] => Ok(read_page(connection, source_key, *only)?
            .map_or(TitleMatch::Absent, TitleMatch::Unique)),
        _ => Ok(TitleMatch::Ambiguous(ids)),
    }
}

fn page_ids_titled(
    connection: &Connection,
    source_key: i64,
    title: &str,
) -> Result<Vec<PageId>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT page_id FROM page WHERE source_id = ?1 AND title_key = ?2 ORDER BY page_id",
        )
        .map_err(StoreError::database(Operation::Read))?;
    let rows = statement
        .query_map(params![source_key, title], |row| row.get::<_, i64>(0))
        .map_err(StoreError::database(Operation::Read))?;
    rows.map(|row| stored_page_id(row.map_err(StoreError::database(Operation::Read))?))
        .collect()
}

/// The form in which titles are compared: without a `#fragment`, with every
/// run of spaces and underscores as one space and none at the ends, and with
/// the first letter in upper case when that is a single character.
///
/// Only the first character of the whole title is folded. A lower-case first
/// letter after a namespace prefix (`Category:foo`) is not, because a prefix
/// cannot be told from a colon in a main-namespace title without the source's
/// namespace list.
pub(super) fn title_key(title: &str) -> String {
    let name = title.split('#').next().unwrap_or_default();
    let spaced = name
        .split(|character: char| character == '_' || character.is_whitespace())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut characters = spaced.chars();
    let Some(first) = characters.next() else {
        return spaced;
    };
    let mut upper = first.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(single), None) => {
            let mut key = String::from(single);
            key.push_str(characters.as_str());
            key
        }
        _ => spaced,
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Fixture, import, page, page_id};
    use super::*;
    use crate::wiki::ImportedPage;

    fn titled(id: u64, title: &str) -> ImportedPage {
        let mut result = page(id, id * 10, "text");
        result.title = PageTitle::new(title.to_owned()).unwrap();
        result
    }

    fn redirect(id: u64, title: &str, target: &str) -> ImportedPage {
        let mut result = titled(id, title);
        result.redirect_target = PageTitle::new(target.to_owned());
        result
    }

    fn store_with(fixture: &Fixture, pages: &[ImportedPage]) -> Store {
        let mut store = fixture.open();
        import(&mut store, &fixture.source, pages);
        store
    }

    fn ids(redirects: &[PageId]) -> Vec<u64> {
        redirects.iter().map(|id| id.get()).collect()
    }

    #[test]
    fn x() {
        assert_eq!(title_key("Dragon_axe"), "Dragon axe");
        assert_eq!(title_key("  Dragon \t__ axe  "), "Dragon axe");
        assert_eq!(title_key("Bank#Notes"), "Bank");
        assert_eq!(title_key("Category:Tools"), "Category:Tools");
        assert_eq!(title_key("axe"), "Axe");
        assert_eq!(title_key("aXE"), "AXE");
        assert_eq!(title_key("élan vital"), "Élan vital");
        assert_eq!(title_key("ßeta"), "ßeta");
        assert_eq!(title_key("1 axe"), "1 axe");
        assert_eq!(title_key("Category:foo"), "Category:foo");
        assert_eq!(title_key("#Local"), "");
        assert_eq!(title_key(" _ "), "");
    }

    #[test]
    fn an_exact_title_finds_its_page() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(1, "Axe"), titled(2, "Pick")]);
        let TitleMatch::Unique(found) = store.page_by_title(&fixture.source, "Pick").unwrap()
        else {
            panic!("expected a unique match");
        };
        assert_eq!(found.id, page_id(2));
    }

    #[test]
    fn spacing_and_fragments_do_not_change_which_page_a_title_names() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(1, "Dragon axe")]);
        for query in ["Dragon_axe", " Dragon  axe ", "Dragon axe#Uses"] {
            assert!(
                matches!(
                    store.page_by_title(&fixture.source, query).unwrap(),
                    TitleMatch::Unique(_)
                ),
                "{query}"
            );
        }
    }

    #[test]
    fn only_the_first_letter_is_case_insensitive() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(1, "Axe")]);
        assert!(matches!(
            store.page_by_title(&fixture.source, "axe").unwrap(),
            TitleMatch::Unique(_)
        ));
        for query in ["AXE", "aXe", "Ax", "Axe s", "", "#Axe"] {
            assert!(
                matches!(
                    store.page_by_title(&fixture.source, query).unwrap(),
                    TitleMatch::Absent
                ),
                "{query}"
            );
        }
    }

    #[test]
    fn a_stored_title_that_needs_normalising_is_found_by_every_spelling() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(1, "dragon__axe  (item)")]);
        for query in [
            "dragon__axe  (item)",
            "Dragon axe (item)",
            "dragon_axe_(item)",
            " Dragon  axe (item)#Uses",
        ] {
            let TitleMatch::Unique(found) = store.page_by_title(&fixture.source, query).unwrap()
            else {
                panic!("{query:?} should find the page");
            };
            // The raw title is kept for display and citation.
            assert_eq!(found.title.as_str(), "dragon__axe  (item)");
        }
    }

    #[test]
    fn redirects_and_targets_meet_whatever_their_spelling() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[
                redirect(1, "old_name", "new__name#Section"),
                titled(2, "new name"),
            ],
        );
        assert!(matches!(
            store.resolve(&fixture.source, "Old name").unwrap(),
            Resolution::Found { page, redirects } if page.id == page_id(2) && redirects.len() == 1
        ));
    }

    #[test]
    fn metacharacters_in_a_title_are_matched_literally() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(1, "Axe")]);
        for query in ["%", "A%", "A_e", "Axe' OR '1'='1", "\"Axe\"", "Axe*"] {
            assert!(
                matches!(
                    store.page_by_title(&fixture.source, query).unwrap(),
                    TitleMatch::Absent
                ),
                "{query}"
            );
        }
    }

    #[test]
    fn the_same_title_in_another_source_is_not_found() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(1, "Axe")]);
        let other = SourceId::new(
            "Other Wiki".to_owned(),
            None,
            "https://other.example/wiki/Main_Page".to_owned(),
        )
        .unwrap();
        assert!(matches!(
            store.page_by_title(&other, "Axe").unwrap(),
            TitleMatch::Absent
        ));
        assert!(matches!(
            store.resolve(&other, "Axe").unwrap(),
            Resolution::Absent
        ));
    }

    #[test]
    fn two_stored_pages_with_one_title_are_reported_not_chosen() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(7, "Axe"), titled(3, "Axe")]);
        let TitleMatch::Ambiguous(pages) = store.page_by_title(&fixture.source, "Axe").unwrap()
        else {
            panic!("expected an ambiguous match");
        };
        assert_eq!(ids(&pages), [3, 7]);
        assert!(matches!(
            store.resolve(&fixture.source, "Axe").unwrap(),
            Resolution::Ambiguous { redirects, .. } if redirects.is_empty()
        ));
    }

    #[test]
    fn titles_that_differ_only_in_the_first_letter_are_ambiguous_not_merged() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(1, "Axe"), titled(2, "axe")]);
        assert!(matches!(
            store.page_by_title(&fixture.source, "Axe").unwrap(),
            TitleMatch::Ambiguous(pages) if ids(&pages) == [1, 2]
        ));
    }

    #[test]
    fn a_page_that_is_not_a_redirect_resolves_to_itself() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[titled(1, "Axe")]);
        let Resolution::Found { page, redirects } = store.resolve(&fixture.source, "Axe").unwrap()
        else {
            panic!("expected a page");
        };
        assert_eq!(page.id, page_id(1));
        assert_eq!(redirects.len(), 0);
    }

    #[test]
    fn an_unknown_title_or_source_resolves_to_absent() {
        let fixture = Fixture::new();
        let empty = fixture.open();
        assert!(matches!(
            empty.resolve(&fixture.source, "Axe").unwrap(),
            Resolution::Absent
        ));
        let store = store_with(&fixture, &[titled(1, "Axe")]);
        assert!(matches!(
            store.resolve(&fixture.source, "Pick").unwrap(),
            Resolution::Absent
        ));
    }

    #[test]
    fn a_redirect_leads_to_its_target_and_records_the_hop() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[
                redirect(1, "Bronze axes", "Bronze axe"),
                titled(2, "Bronze axe"),
            ],
        );
        let Resolution::Found { page, redirects } =
            store.resolve(&fixture.source, "Bronze axes").unwrap()
        else {
            panic!("expected a page");
        };
        assert_eq!(page.id, page_id(2));
        assert_eq!(ids(&redirects), [1]);
    }

    #[test]
    fn a_redirect_target_with_a_fragment_or_underscores_still_resolves() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[
                redirect(1, "Gp", "Coins_(item)#Uses"),
                titled(2, "Coins (item)"),
            ],
        );
        assert!(matches!(
            store.resolve(&fixture.source, "Gp").unwrap(),
            Resolution::Found { page, .. } if page.id == page_id(2)
        ));
    }

    #[test]
    fn a_chain_of_redirects_is_followed_in_order() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[redirect(1, "A", "B"), redirect(2, "B", "C"), titled(3, "C")],
        );
        let Resolution::Found { page, redirects } = store.resolve(&fixture.source, "A").unwrap()
        else {
            panic!("expected a page");
        };
        assert_eq!(page.id, page_id(3));
        assert_eq!(ids(&redirects), [1, 2]);
    }

    #[test]
    fn a_redirect_to_a_missing_page_names_the_missing_target() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[redirect(1, "A", "Missing page")]);
        let Resolution::MissingTarget { target, redirects } =
            store.resolve(&fixture.source, "A").unwrap()
        else {
            panic!("expected a missing target");
        };
        assert_eq!(target.as_str(), "Missing page");
        assert_eq!(ids(&redirects), [1]);
    }

    #[test]
    fn a_missing_target_further_along_a_chain_reports_the_whole_path() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[redirect(1, "A", "B"), redirect(2, "B", "Gone")]);
        let Resolution::MissingTarget { target, redirects } =
            store.resolve(&fixture.source, "A").unwrap()
        else {
            panic!("expected a missing target");
        };
        assert_eq!(target.as_str(), "Gone");
        assert_eq!(ids(&redirects), [1, 2]);
    }

    #[test]
    fn redirect_loops_are_detected_including_a_page_redirecting_to_itself() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[
                redirect(1, "A", "B"),
                redirect(2, "B", "A"),
                redirect(3, "Self", "Self"),
            ],
        );
        let Resolution::Loop { redirects } = store.resolve(&fixture.source, "A").unwrap() else {
            panic!("expected a loop");
        };
        assert_eq!(ids(&redirects), [1, 2]);
        let Resolution::Loop { redirects } = store.resolve(&fixture.source, "Self").unwrap() else {
            panic!("expected a loop");
        };
        assert_eq!(ids(&redirects), [3]);
    }

    #[test]
    fn a_chain_longer_than_the_bound_stops_without_a_page() {
        let fixture = Fixture::new();
        let length = u64::try_from(MAX_REDIRECTS).unwrap();
        let mut pages: Vec<ImportedPage> = (1..=length + 1)
            .map(|id| redirect(id, &format!("R{id}"), &format!("R{}", id + 1)))
            .collect();
        pages.push(titled(length + 2, &format!("R{}", length + 2)));
        let store = store_with(&fixture, &pages);
        let Resolution::TooManyRedirects { redirects } =
            store.resolve(&fixture.source, "R1").unwrap()
        else {
            panic!("expected the bound to apply");
        };
        assert_eq!(redirects.len(), MAX_REDIRECTS);
    }

    #[test]
    fn a_chain_exactly_at_the_bound_still_resolves() {
        let fixture = Fixture::new();
        let length = u64::try_from(MAX_REDIRECTS).unwrap();
        let mut pages: Vec<ImportedPage> = (1..=length)
            .map(|id| redirect(id, &format!("R{id}"), &format!("R{}", id + 1)))
            .collect();
        pages.push(titled(length + 1, &format!("R{}", length + 1)));
        let store = store_with(&fixture, &pages);
        assert!(matches!(
            store.resolve(&fixture.source, "R1").unwrap(),
            Resolution::Found { redirects, .. } if redirects.len() == MAX_REDIRECTS
        ));
    }

    #[test]
    fn a_title_shared_by_two_pages_on_the_way_is_reported_with_the_hops_so_far() {
        let fixture = Fixture::new();
        let store = store_with(
            &fixture,
            &[redirect(1, "A", "B"), titled(2, "B"), titled(3, "B")],
        );
        let Resolution::Ambiguous { pages, redirects } =
            store.resolve(&fixture.source, "A").unwrap()
        else {
            panic!("expected ambiguity");
        };
        assert_eq!(ids(&pages), [2, 3]);
        assert_eq!(ids(&redirects), [1]);
    }

    #[test]
    fn lookups_see_a_replaced_revision_and_survive_reopening() {
        let fixture = Fixture::new();
        let mut store = store_with(&fixture, &[titled(1, "A"), titled(2, "B")]);
        let mut newer = redirect(1, "A", "B");
        newer.revision = crate::wiki::RevisionId::new(11).unwrap();
        import(&mut store, &fixture.source, &[newer]);
        drop(store);
        let reopened = fixture.open();
        assert!(matches!(
            reopened.resolve(&fixture.source, "A").unwrap(),
            Resolution::Found { page, redirects } if page.id == page_id(2) && redirects.len() == 1
        ));
    }

    #[test]
    fn debug_output_of_a_resolution_omits_titles_and_text() {
        let fixture = Fixture::new();
        let store = store_with(&fixture, &[redirect(1, "Secret source", "Secret target")]);
        let shown = format!(
            "{:?} {:?}",
            store.resolve(&fixture.source, "Secret source").unwrap(),
            store
                .page_by_title(&fixture.source, "Secret source")
                .unwrap()
        );
        assert!(!shown.contains("Secret"), "{shown}");
    }
}
