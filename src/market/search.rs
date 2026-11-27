//! Pure item lookup over a Hardcore item mapping.
//!
//! No network, clock or MCP types: callers pass the mapping they already
//! fetched. A match shows that an item exists in the mapping. It says nothing
//! about whether the item is currently traded.
//!
//! # Ranking
//!
//! Matches are ordered by [`MatchKind`] (`Id`, then exact name, then prefix,
//! then substring), and ties are broken by lowercased name, then name, then
//! item id, so the order does not depend on the order of the mapping. Names are
//! compared case-insensitively using Unicode lowercasing.

use std::cmp::Ordering;

use thiserror::Error;

use crate::domain::ItemId;

use super::model::ItemInfo;

/// Shortest accepted name query, in characters. One character matches too much
/// of the mapping to be useful.
pub const MIN_NAME_CHARS: usize = 2;
/// Longest accepted name query, in characters.
pub const MAX_NAME_CHARS: usize = 100;
/// Number of matches returned when the caller does not choose.
pub const DEFAULT_LIMIT: usize = 10;
/// Largest number of matches a caller may ask for.
pub const MAX_LIMIT: usize = 25;

/// Why a lookup request was rejected before any data was fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum QueryError {
    /// Neither an item id nor a name was given.
    #[error("provide either `item_id` or `name`")]
    Missing,
    /// Both an item id and a name were given.
    #[error("provide only one of `item_id` and `name`")]
    Conflicting,
    /// The name, after trimming, is shorter than [`MIN_NAME_CHARS`].
    #[error("`name` must be at least {MIN_NAME_CHARS} characters after trimming")]
    NameTooShort,
    /// The name, after trimming, is longer than [`MAX_NAME_CHARS`].
    #[error("`name` must be at most {MAX_NAME_CHARS} characters")]
    NameTooLong,
    /// The limit is below 1 or above [`MAX_LIMIT`].
    #[error("`limit` must be between 1 and {MAX_LIMIT}")]
    LimitOutOfRange,
}

/// A validated, trimmed name query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameQuery(String);

impl NameQuery {
    /// Trims `text` and checks its length in characters.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::NameTooShort`] or [`QueryError::NameTooLong`].
    pub fn new(text: &str) -> Result<Self, QueryError> {
        let trimmed = text.trim();
        match trimmed.chars().count() {
            count if count < MIN_NAME_CHARS => Err(QueryError::NameTooShort),
            count if count > MAX_NAME_CHARS => Err(QueryError::NameTooLong),
            _ => Ok(Self(trimmed.to_owned())),
        }
    }
}

/// What to look up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemQuery {
    /// An exact item id.
    Id(ItemId),
    /// A name, matched exactly, by prefix or as a substring.
    Name(NameQuery),
}

impl ItemQuery {
    /// Builds a query from the two optional request fields, which must be
    /// given exactly one at a time.
    ///
    /// # Errors
    ///
    /// Returns a [`QueryError`] if neither or both are given, or if the name
    /// is not valid.
    pub fn from_parts(item_id: Option<ItemId>, name: Option<&str>) -> Result<Self, QueryError> {
        match (item_id, name) {
            (None, None) => Err(QueryError::Missing),
            (Some(_), Some(_)) => Err(QueryError::Conflicting),
            (Some(id), None) => Ok(Self::Id(id)),
            (None, Some(name)) => NameQuery::new(name).map(Self::Name),
        }
    }
}

/// The maximum number of matches to return, between 1 and [`MAX_LIMIT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultLimit(usize);

impl ResultLimit {
    /// The limit used when the caller gives none.
    pub const DEFAULT: Self = Self(DEFAULT_LIMIT);

    /// Checks that `limit` is at least 1 and at most [`MAX_LIMIT`].
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::LimitOutOfRange`] otherwise.
    pub const fn new(limit: usize) -> Result<Self, QueryError> {
        if limit == 0 || limit > MAX_LIMIT {
            Err(QueryError::LimitOutOfRange)
        } else {
            Ok(Self(limit))
        }
    }
}

/// How a result matched the query. The derived order is the ranking order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchKind {
    /// The item id equals the requested id.
    Id,
    /// The whole name equals the query, ignoring case.
    ExactName,
    /// The name starts with the query, ignoring case.
    Prefix,
    /// The name contains the query, ignoring case.
    Substring,
}

/// One matching item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemMatch {
    /// The mapping entry, unchanged.
    pub item: ItemInfo,
    /// How it matched.
    pub kind: MatchKind,
}

/// The result of a lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutcome {
    /// The best matches, at most the requested limit, in ranking order.
    pub matches: Vec<ItemMatch>,
    /// How many items matched before the limit was applied.
    pub total_matches: usize,
    /// Whether more than one item shares the best match kind, so the best
    /// match alone does not identify a single item.
    pub ambiguous: bool,
}

impl SearchOutcome {
    /// Whether the limit cut off matches.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.total_matches > self.matches.len()
    }
}

/// Looks `query` up in `items`. Pure and deterministic.
#[must_use]
pub fn search(items: &[ItemInfo], query: &ItemQuery, limit: ResultLimit) -> SearchOutcome {
    let mut found: Vec<RankedMatch<'_>> = items
        .iter()
        .filter_map(|item| RankedMatch::new(item, query))
        .collect();
    found.sort_by(RankedMatch::ranking);

    let total_matches = found.len();
    let ambiguous = found.first().is_some_and(|best| {
        found
            .iter()
            .take_while(|other| other.kind == best.kind)
            .nth(1)
            .is_some()
    });
    let matches = found
        .into_iter()
        .take(limit.0)
        .map(|ranked| ItemMatch {
            item: ranked.item.clone(),
            kind: ranked.kind,
        })
        .collect();

    SearchOutcome {
        matches,
        total_matches,
        ambiguous,
    }
}

/// A match with the lowercased name kept for tie-breaking.
struct RankedMatch<'a> {
    item: &'a ItemInfo,
    kind: MatchKind,
    lowercase_name: String,
}

impl<'a> RankedMatch<'a> {
    fn new(item: &'a ItemInfo, query: &ItemQuery) -> Option<Self> {
        let lowercase_name = item.name.to_lowercase();
        let kind = match query {
            ItemQuery::Id(id) => (item.id == *id).then_some(MatchKind::Id),
            ItemQuery::Name(name) => name_match_kind(&lowercase_name, &name.0.to_lowercase()),
        }?;
        Some(Self {
            item,
            kind,
            lowercase_name,
        })
    }

    fn ranking(&self, other: &Self) -> Ordering {
        self.kind
            .cmp(&other.kind)
            .then_with(|| self.lowercase_name.cmp(&other.lowercase_name))
            .then_with(|| self.item.name.cmp(&other.item.name))
            .then_with(|| self.item.id.cmp(&other.item.id))
    }
}

fn name_match_kind(lowercase_name: &str, lowercase_query: &str) -> Option<MatchKind> {
    if lowercase_name == lowercase_query {
        Some(MatchKind::ExactName)
    } else if lowercase_name.starts_with(lowercase_query) {
        Some(MatchKind::Prefix)
    } else if lowercase_name.contains(lowercase_query) {
        Some(MatchKind::Substring)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u32, name: &str) -> ItemInfo {
        ItemInfo {
            id: ItemId(id),
            name: name.to_owned(),
            members: None,
            buy_limit: None,
            value: None,
            low_alchemy: None,
            high_alchemy: None,
        }
    }

    fn name_query(text: &str) -> ItemQuery {
        ItemQuery::Name(NameQuery::new(text).unwrap())
    }

    fn limit(count: usize) -> ResultLimit {
        ResultLimit::new(count).unwrap()
    }

    fn ids(outcome: &SearchOutcome) -> Vec<u32> {
        outcome.matches.iter().map(|m| m.item.id.0).collect()
    }

    fn kinds(outcome: &SearchOutcome) -> Vec<MatchKind> {
        outcome.matches.iter().map(|m| m.kind).collect()
    }

    fn mapping() -> Vec<ItemInfo> {
        vec![
            item(2, "Cannonball"),
            item(3, "Granite cannonball"),
            item(4, "Cannonball mould"),
            item(5, "Cannon base"),
            item(6, "Dragon scimitar"),
        ]
    }

    #[test]
    fn exact_name_ranks_before_prefix_and_substring_matches() {
        let outcome = search(&mapping(), &name_query("cannonball"), limit(10));

        assert_eq!(ids(&outcome), [2, 4, 3]);
        assert_eq!(
            kinds(&outcome),
            [
                MatchKind::ExactName,
                MatchKind::Prefix,
                MatchKind::Substring
            ]
        );
        assert_eq!(outcome.total_matches, 3);
    }

    #[test]
    fn name_matching_ignores_case_and_surrounding_whitespace() {
        let outcome = search(&mapping(), &name_query("  CANNONBALL "), limit(10));

        assert_eq!(outcome.matches[0].item.id, ItemId(2));
        assert_eq!(outcome.matches[0].kind, MatchKind::ExactName);
    }

    #[test]
    fn an_id_lookup_returns_that_item_alone() {
        let outcome = search(&mapping(), &ItemQuery::Id(ItemId(5)), limit(10));

        assert_eq!(ids(&outcome), [5]);
        assert_eq!(kinds(&outcome), [MatchKind::Id]);
        assert!(!outcome.ambiguous);
    }

    #[test]
    fn no_match_is_an_empty_outcome_not_an_error() {
        let by_name = search(&mapping(), &name_query("zzz"), limit(10));
        let by_id = search(&mapping(), &ItemQuery::Id(ItemId(999)), limit(10));

        for outcome in [by_name, by_id] {
            assert_eq!(outcome.matches.len(), 0);
            assert_eq!(outcome.total_matches, 0);
            assert!(!outcome.ambiguous);
            assert!(!outcome.truncated());
        }
    }

    #[test]
    fn a_single_best_match_is_not_ambiguous_even_with_weaker_matches() {
        let outcome = search(&mapping(), &name_query("cannonball"), limit(10));

        assert!(!outcome.ambiguous);
    }

    #[test]
    fn several_matches_of_the_best_kind_are_ambiguous() {
        let outcome = search(&mapping(), &name_query("cannon"), limit(10));

        assert_eq!(
            kinds(&outcome),
            [
                MatchKind::Prefix,
                MatchKind::Prefix,
                MatchKind::Prefix,
                MatchKind::Substring
            ]
        );
        assert!(outcome.ambiguous);
    }

    #[test]
    fn duplicate_exact_names_are_ambiguous_and_ordered_by_id() {
        let items = vec![item(9, "Twin"), item(7, "Twin"), item(8, "TWIN")];

        let outcome = search(&items, &name_query("twin"), limit(10));

        // "TWIN" sorts after "Twin" on the original-case tie-break.
        assert_eq!(ids(&outcome), [8, 7, 9]);
        assert!(outcome.ambiguous);
    }

    #[test]
    fn order_does_not_depend_on_the_order_of_the_mapping() {
        let forward = search(&mapping(), &name_query("cannon"), limit(10));
        let mut reversed_items = mapping();
        reversed_items.reverse();

        let reversed = search(&reversed_items, &name_query("cannon"), limit(10));

        assert_eq!(forward, reversed);
    }

    #[test]
    fn the_limit_keeps_the_best_matches_and_reports_truncation() {
        let outcome = search(&mapping(), &name_query("cannon"), limit(2));

        assert_eq!(ids(&outcome), [5, 2]);
        assert_eq!(outcome.total_matches, 4);
        assert!(outcome.truncated());
        assert!(outcome.ambiguous, "ambiguity is judged before the limit");
    }

    #[test]
    fn matches_keep_the_mapping_entry_unchanged_including_missing_metadata() {
        let mut full = item(1, "Example ammo");
        full.members = Some(false);
        full.buy_limit = Some(11_000);
        let bare = item(2, "Example bare");

        let outcome = search(
            &[full.clone(), bare.clone()],
            &name_query("example"),
            limit(10),
        );

        assert_eq!(outcome.matches[0].item, full);
        assert_eq!(outcome.matches[1].item, bare);
        assert_eq!(outcome.matches[1].item.buy_limit, None);
    }

    #[test]
    fn name_queries_are_trimmed_and_length_checked_in_characters() {
        assert_eq!(NameQuery::new("a "), Err(QueryError::NameTooShort));
        assert_eq!(NameQuery::new("   "), Err(QueryError::NameTooShort));
        assert!(NameQuery::new("ab").is_ok());
        assert!(NameQuery::new(&"é".repeat(MAX_NAME_CHARS)).is_ok());
        assert_eq!(
            NameQuery::new(&"é".repeat(MAX_NAME_CHARS + 1)),
            Err(QueryError::NameTooLong)
        );
    }

    #[test]
    fn exactly_one_of_id_and_name_is_required() {
        assert_eq!(ItemQuery::from_parts(None, None), Err(QueryError::Missing));
        assert_eq!(
            ItemQuery::from_parts(Some(ItemId(1)), Some("ab")),
            Err(QueryError::Conflicting)
        );
        assert_eq!(
            ItemQuery::from_parts(Some(ItemId(1)), None),
            Ok(ItemQuery::Id(ItemId(1)))
        );
        assert_eq!(
            ItemQuery::from_parts(None, Some(" ab ")),
            Ok(ItemQuery::Name(NameQuery("ab".to_owned())))
        );
    }

    #[test]
    fn the_limit_must_be_between_one_and_the_maximum() {
        assert_eq!(ResultLimit::new(0), Err(QueryError::LimitOutOfRange));
        assert_eq!(
            ResultLimit::new(MAX_LIMIT + 1),
            Err(QueryError::LimitOutOfRange)
        );
        assert!(ResultLimit::new(1).is_ok());
        assert!(ResultLimit::new(MAX_LIMIT).is_ok());
        assert_eq!(ResultLimit::DEFAULT, limit(DEFAULT_LIMIT));
    }
}
