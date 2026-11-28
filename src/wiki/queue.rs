//! Rules for an explicit manual page request queue for one Wiki source.
//!
//! The caller chooses the source and supplies committed import observations.
//! The queue performs no acquisition, redirect traversal, or persistence.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU64;

use thiserror::Error;

const MAX_TITLE_BYTES: usize = 255;
const MAX_REASON_BYTES: usize = 512;
const MAX_REASONS: usize = 8;
const MAX_REQUESTS: usize = 1_000;

/// A syntactically valid request title. Its existence remains unverified.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestTitle(String);

impl RequestTitle {
    /// Normalises underscores and spaces, capitalises the first letter, and
    /// canonicalises known English namespace prefixes case-insensitively.
    /// Unknown prefixes remain ordinary title text. The rest of the title
    /// retains its case, including upper-case mappings that would expand.
    ///
    /// # Errors
    ///
    /// Rejects empty titles, unsafe characters, path navigation and titles
    /// over 255 bytes before or after normalisation.
    pub fn parse(input: &str) -> Result<Self, QueueError> {
        if input.len() > MAX_TITLE_BYTES {
            return Err(QueueError::TitleTooLong);
        }
        if input.chars().any(forbidden_title_character) || input.contains("://") {
            return Err(QueueError::InvalidTitle);
        }

        let normalised = input
            .split([' ', '_'])
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let (namespace, name) = split_namespace(&normalised);
        if name.is_empty()
            || name.starts_with(['/', ':'])
            || name
                .split('/')
                .any(|component| matches!(component, "." | ".."))
        {
            return Err(QueueError::InvalidTitle);
        }

        let title = format!("{namespace}{}", capitalise(name));
        if title.len() > MAX_TITLE_BYTES {
            return Err(QueueError::TitleTooLong);
        }
        Ok(Self(title))
    }

    /// Returns the canonical title for deliberate display or lookup.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RequestTitle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "RequestTitle({} bytes)", self.0.len())
    }
}

const fn forbidden_title_character(character: char) -> bool {
    character.is_control()
        || (character.is_whitespace() && character != ' ')
        || matches!(
            character,
            '#' | '[' | ']' | '{' | '}' | '<' | '>' | '|' | '%' | '\\'
                | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

fn split_namespace(title: &str) -> (&str, &str) {
    const NAMESPACES: &[&str] = &[
        "Talk:",
        "User:",
        "User talk:",
        "File:",
        "File talk:",
        "MediaWiki:",
        "MediaWiki talk:",
        "Template:",
        "Template talk:",
        "Help:",
        "Help talk:",
        "Category:",
        "Category talk:",
        "Module:",
        "Module talk:",
    ];

    if let Some((prefix, name)) = title.split_once(':') {
        let prefix = prefix.trim_end();
        if let Some(namespace) = NAMESPACES.iter().copied().find(|namespace| {
            namespace
                .strip_suffix(':')
                .is_some_and(|label| label.eq_ignore_ascii_case(prefix))
        }) {
            return (namespace, name.trim_start());
        }
    }
    ("", title)
}

fn capitalise(name: &str) -> String {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    let mut uppercase = first.to_uppercase();
    let upper = uppercase.next().unwrap_or(first);
    let initial = if uppercase.next().is_none() {
        upper
    } else {
        first
    };
    format!("{initial}{}", characters.as_str())
}

/// An invalid operation on the request queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum QueueError {
    /// The title exceeds the input limit.
    #[error("request title exceeds 255 bytes")]
    TitleTooLong,
    /// The title is empty or cannot safely be used as an export title.
    #[error("invalid request title")]
    InvalidTitle,
    /// The reason is empty, oversized, or contains unsafe characters.
    #[error("request reason must be a non-empty line of at most 512 bytes")]
    InvalidReason,
    /// The request already contains the maximum number of distinct reasons.
    #[error("request already has eight distinct reasons")]
    TooManyReasons,
    /// The queue is full.
    #[error("request queue already has 1000 entries")]
    TooManyRequests,
    /// The queue does not contain the requested title.
    #[error("unknown request")]
    UnknownRequest,
    /// A redirect leads directly back to the request itself.
    #[error("request cannot redirect to itself")]
    RedirectCycle,
}

/// Current progress on a requested page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestStatus {
    /// A committed matching import is still needed.
    Requested,
    /// A matching revision was successfully imported.
    Satisfied(NonZeroU64),
    /// The user explicitly dismissed the request.
    Dismissed,
    /// A revision strictly newer than the known revision must be imported.
    RefreshRequested(NonZeroU64),
    /// The resolved redirect destination still needs a matching import.
    UnresolvedTarget {
        /// Destination reported by the caller's redirect lookup.
        target: RequestTitle,
        /// Minimum exclusive revision for a refresh.
        newer_than: Option<NonZeroU64>,
    },
}

impl RequestStatus {
    const fn refresh_baseline(&self) -> Option<NonZeroU64> {
        match self {
            Self::RefreshRequested(revision) => Some(*revision),
            Self::UnresolvedTarget { newer_than, .. } => *newer_than,
            Self::Requested | Self::Satisfied(_) | Self::Dismissed => None,
        }
    }
}

/// The reasons, priority and progress for one canonical title.
pub struct Request {
    reasons: BTreeSet<String>,
    priority: u8,
    status: RequestStatus,
}

impl Request {
    /// Distinct user-supplied reasons in lexical order.
    pub fn reasons(&self) -> impl ExactSizeIterator<Item = &str> {
        self.reasons.iter().map(String::as_str)
    }

    /// Highest priority supplied so far. Absent priority is zero.
    #[must_use]
    pub const fn priority(&self) -> u8 {
        self.priority
    }

    /// Current progress on this request.
    #[must_use]
    pub const fn status(&self) -> &RequestStatus {
        &self.status
    }

    fn merge(&mut self, reason: &str, priority: u8) -> Result<(), QueueError> {
        if !self.reasons.contains(reason) {
            if self.reasons.len() >= MAX_REASONS {
                return Err(QueueError::TooManyReasons);
            }
            self.reasons.insert(reason.to_owned());
        }
        self.priority = self.priority.max(priority);
        Ok(())
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Request")
            .field("reason_count", &self.reasons.len())
            .field("priority", &self.priority)
            .field("status", &self.status)
            .finish()
    }
}

/// An in-memory queue scoped to a single source chosen by its owner.
///
/// Stores at most 1000 entries, including entries previously satisfied or
/// dismissed. A title has up to eight distinct reasons of 512 bytes each.
#[derive(Debug, Default)]
pub struct RequestQueue {
    entries: BTreeMap<RequestTitle, Request>,
}

impl RequestQueue {
    /// Adds or merges an explicit request. A dismissed request is reopened.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid reason or exceeded queue limits.
    pub fn request(
        &mut self,
        title: RequestTitle,
        reason: &str,
        priority: Option<u8>,
    ) -> Result<(), QueueError> {
        let entry = self.add_or_merge(title, reason, priority)?;
        if entry.status == RequestStatus::Dismissed {
            entry.status = RequestStatus::Requested;
        }
        Ok(())
    }

    /// Requests a revision strictly newer than the specified known revision.
    /// For a known redirect, the baseline is the destination's revision.
    /// Repeat refreshes retain the highest baseline.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid reason or exceeded queue limits.
    pub fn request_refresh(
        &mut self,
        title: RequestTitle,
        reason: &str,
        priority: Option<u8>,
        revision: NonZeroU64,
    ) -> Result<(), QueueError> {
        let entry = self.add_or_merge(title, reason, priority)?;
        let known_revision = match &entry.status {
            RequestStatus::Satisfied(imported) => Some(*imported),
            status => status.refresh_baseline(),
        };
        let baseline = known_revision.map_or(revision, |known| known.max(revision));
        match &mut entry.status {
            RequestStatus::UnresolvedTarget { newer_than, .. } => *newer_than = Some(baseline),
            status => *status = RequestStatus::RefreshRequested(baseline),
        }
        Ok(())
    }

    /// Returns the request for a canonical title.
    #[must_use]
    pub fn get(&self, title: &RequestTitle) -> Option<&Request> {
        self.entries.get(title)
    }

    /// Lists all requests in canonical title order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&RequestTitle, &Request)> {
        self.entries.iter()
    }

    /// Dismisses an existing request.
    ///
    /// # Errors
    ///
    /// Returns an error when the title has no request.
    pub fn dismiss(&mut self, title: &RequestTitle) -> Result<(), QueueError> {
        self.get_mut(title)?.status = RequestStatus::Dismissed;
        Ok(())
    }

    /// Records a missing redirect destination for an outstanding request.
    /// An already satisfied or dismissed request is left alone. This operation
    /// neither traverses redirects nor adds a separate request for the target.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown request or a self-redirect.
    pub fn mark_unresolved(
        &mut self,
        title: &RequestTitle,
        target: RequestTitle,
    ) -> Result<(), QueueError> {
        let entry = self.get_mut(title)?;
        if title == &target {
            return Err(QueueError::RedirectCycle);
        }
        if !matches!(
            entry.status,
            RequestStatus::Satisfied(_) | RequestStatus::Dismissed
        ) {
            entry.status = RequestStatus::UnresolvedTarget {
                target,
                newer_than: entry.status.refresh_baseline(),
            };
        }
        Ok(())
    }

    /// Applies a committed import of a usable page. Returns whether it satisfied
    /// the request. The caller supplies an observation only after successful
    /// commit and checks that the imported page is sufficient for the request.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested title has no request.
    pub fn record_import(
        &mut self,
        title: &RequestTitle,
        imported: &RequestTitle,
        revision: NonZeroU64,
    ) -> Result<bool, QueueError> {
        let entry = self.get_mut(title)?;
        let expected = match &entry.status {
            RequestStatus::Requested | RequestStatus::RefreshRequested(_) => title,
            RequestStatus::UnresolvedTarget { target, .. } => target,
            RequestStatus::Satisfied(_) | RequestStatus::Dismissed => return Ok(false),
        };
        if imported != expected
            || entry
                .status
                .refresh_baseline()
                .is_some_and(|baseline| revision <= baseline)
        {
            return Ok(false);
        }
        entry.status = RequestStatus::Satisfied(revision);
        Ok(true)
    }

    fn get_mut(&mut self, title: &RequestTitle) -> Result<&mut Request, QueueError> {
        self.entries
            .get_mut(title)
            .ok_or(QueueError::UnknownRequest)
    }

    fn add_or_merge(
        &mut self,
        title: RequestTitle,
        reason: &str,
        priority: Option<u8>,
    ) -> Result<&mut Request, QueueError> {
        let reason = validated_reason(reason)?;
        let is_full = self.entries.len() >= MAX_REQUESTS;
        let priority = priority.unwrap_or_default();
        match self.entries.entry(title) {
            Entry::Occupied(entry) => {
                let entry = entry.into_mut();
                entry.merge(reason, priority)?;
                Ok(entry)
            }
            Entry::Vacant(_) if is_full => Err(QueueError::TooManyRequests),
            Entry::Vacant(entry) => Ok(entry.insert(Request {
                reasons: BTreeSet::from([reason.to_owned()]),
                priority,
                status: RequestStatus::Requested,
            })),
        }
    }
}

fn validated_reason(input: &str) -> Result<&str, QueueError> {
    let reason = input.trim();
    if input.len() > MAX_REASON_BYTES
        || reason.is_empty()
        || input
            .chars()
            .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
    {
        return Err(QueueError::InvalidReason);
    }
    Ok(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(text: &str) -> RequestTitle {
        RequestTitle::parse(text).unwrap()
    }

    #[test]
    fn title_normalisation_preserves_case_after_the_first_letter() {
        assert_eq!(title("  café__Ledger_ ").as_str(), "Café Ledger");
        assert_ne!(title("café Ledger"), title("Café ledger"));
        assert_eq!(
            title("template: iron_ledger").as_str(),
            "Template:Iron ledger"
        );
        assert_eq!(
            title("field note: iron_ledger").as_str(),
            "Field note: iron ledger"
        );
    }

    #[test]
    fn unsafe_titles_are_rejected_without_echoing_the_input() {
        for input in [
            "__",
            "Page\nOther",
            "Page\u{2028}Other",
            "Page#Section",
            "[[Page]]",
            "{{Page}}",
            "Page|Other",
            "<Page>",
            "https://example.test",
            "Page%0aOther",
            "Talk:__",
            "Page/../Other",
        ] {
            let error = RequestTitle::parse(input).unwrap_err();
            assert!(!error.to_string().contains(input));
        }
        assert!(RequestTitle::parse(&"x".repeat(MAX_TITLE_BYTES)).is_ok());
        assert!(RequestTitle::parse(&"x".repeat(MAX_TITLE_BYTES + 1)).is_err());
    }

    #[test]
    fn repeated_requests_merge_reasons_and_keep_the_highest_priority() {
        let mut queue = RequestQueue::default();
        queue
            .request(title("lost_ledger"), "Page is missing", Some(2))
            .unwrap();
        queue
            .request(title("Lost ledger"), "Content is incomplete", Some(4))
            .unwrap();
        queue
            .request(title("lost ledger"), "Page is missing", Some(1))
            .unwrap();

        let requests = queue.iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 1);
        let (requested_title, entry) = requests[0];
        assert_eq!(requested_title.as_str(), "Lost ledger");
        assert_eq!(entry.priority(), 4);
        assert_eq!(
            entry.reasons().collect::<Vec<_>>(),
            ["Content is incomplete", "Page is missing"]
        );
        assert_eq!(entry.status(), &RequestStatus::Requested);
    }

    #[test]
    fn reason_limit_errors_leave_existing_requests_unchanged() {
        let mut queue = RequestQueue::default();
        let page = title("Lost ledger");
        for number in 0..MAX_REASONS {
            queue
                .request(page.clone(), &format!("Reason {number}"), Some(1))
                .unwrap();
        }
        assert_eq!(
            queue.request(page.clone(), "One more", Some(9)),
            Err(QueueError::TooManyReasons)
        );
        assert_eq!(queue.get(&page).unwrap().priority(), 1);
        assert_eq!(queue.get(&page).unwrap().reasons().count(), MAX_REASONS);
    }

    #[test]
    fn full_queue_still_accepts_duplicates() {
        let mut queue = RequestQueue::default();
        for number in 0..MAX_REQUESTS {
            queue
                .request(title(&format!("Ledger {number}")), "Missing", None)
                .unwrap();
        }
        assert_eq!(
            queue.request(title("New ledger"), "Missing", None),
            Err(QueueError::TooManyRequests)
        );
        queue
            .request(title("Ledger 0"), "Missing", Some(7))
            .unwrap();
        assert_eq!(queue.get(&title("Ledger 0")).unwrap().priority(), 7);
    }

    #[test]
    fn dismiss_and_rerequest_are_explicit() {
        let mut queue = RequestQueue::default();
        let page = title("Lost ledger");
        assert_eq!(queue.dismiss(&page), Err(QueueError::UnknownRequest));
        queue.request(page.clone(), "Missing", None).unwrap();
        queue.dismiss(&page).unwrap();
        assert_eq!(
            queue.get(&page).unwrap().status(),
            &RequestStatus::Dismissed
        );
        queue.request(page.clone(), "Needed again", None).unwrap();
        assert_eq!(
            queue.get(&page).unwrap().status(),
            &RequestStatus::Requested
        );
    }

    #[test]
    fn missing_target_stays_visible_and_requires_matching_completion() {
        let mut queue = RequestQueue::default();
        let alias = title("Ledger pointer");
        let target = title("Lost ledger");
        queue.request(alias.clone(), "Missing", None).unwrap();
        queue.mark_unresolved(&alias, target.clone()).unwrap();
        assert_eq!(
            queue.get(&alias).unwrap().status(),
            &RequestStatus::UnresolvedTarget {
                target: target.clone(),
                newer_than: None
            }
        );
        let revision = NonZeroU64::new(17).unwrap();
        assert!(!queue.record_import(&alias, &alias, revision).unwrap());
        assert!(queue.record_import(&alias, &target, revision).unwrap());
        assert_eq!(
            queue.get(&alias).unwrap().status(),
            &RequestStatus::Satisfied(revision)
        );
        assert!(queue.get(&target).is_none());
    }

    #[test]
    fn refresh_after_redirect_requires_a_newer_revision() {
        let mut queue = RequestQueue::default();
        let alias = title("Ledger pointer");
        let target = title("Lost ledger");
        let current = NonZeroU64::new(17).unwrap();
        let older = NonZeroU64::new(16).unwrap();
        let newer = NonZeroU64::new(18).unwrap();
        queue
            .request_refresh(alias.clone(), "Refresh", None, current)
            .unwrap();
        queue.mark_unresolved(&alias, target.clone()).unwrap();

        assert!(!queue.record_import(&alias, &target, older).unwrap());
        assert!(!queue.record_import(&alias, &target, current).unwrap());
        assert!(queue.record_import(&alias, &target, newer).unwrap());
        assert_eq!(
            queue.get(&alias).unwrap().status(),
            &RequestStatus::Satisfied(newer)
        );
    }

    #[test]
    fn self_redirect_does_not_change_request_or_create_a_target() {
        let mut queue = RequestQueue::default();
        let page = title("Ledger loop");
        queue.request(page.clone(), "Missing", None).unwrap();

        assert_eq!(
            queue.mark_unresolved(&page, title("ledger_loop")),
            Err(QueueError::RedirectCycle)
        );
        assert_eq!(
            queue.get(&page).unwrap().status(),
            &RequestStatus::Requested
        );
        assert_eq!(queue.iter().len(), 1);
    }

    #[test]
    fn duplicate_refresh_cannot_lower_the_revision_requirement() {
        let mut queue = RequestQueue::default();
        let page = title("Lost ledger");
        let older = NonZeroU64::new(16).unwrap();
        let current = NonZeroU64::new(17).unwrap();
        queue
            .request_refresh(page.clone(), "Refresh", None, current)
            .unwrap();
        queue
            .request_refresh(page.clone(), "Duplicate", None, older)
            .unwrap();

        assert_eq!(
            queue.get(&page).unwrap().status(),
            &RequestStatus::RefreshRequested(current)
        );
    }

    #[test]
    fn errors_and_debug_redact_request_content() {
        let mut queue = RequestQueue::default();
        let page = title("ExampleMarker title");
        queue
            .request(page.clone(), "ExampleMarker reason", None)
            .unwrap();
        let debug = format!("{queue:?}");
        assert!(!debug.contains("ExampleMarker"));
        let error = queue
            .request(page, "ExampleMarker\nreason", None)
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("ExampleMarker"));
    }
}
