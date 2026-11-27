//! Plain data types for imported Wiki pages.
//!
//! Every string here comes from an export file and is untrusted data. The
//! types check shape and size so that later stages (preparation, storage,
//! tools) can rely on those invariants, but they say nothing about whether the
//! content is true or safe to follow as an instruction.

use std::fmt;
use std::num::NonZeroU64;

use super::timestamp::RevisionTimestamp;

/// Longest accepted page title, site name or database name, in bytes. `MediaWiki`
/// limits titles to 255 bytes; the margin allows a redirect target with a
/// fragment.
const MAX_NAME_BYTES: usize = 1024;
/// Longest accepted site URL, in bytes.
const MAX_URL_BYTES: usize = 2048;
/// Longest accepted exported hash, in characters.
const MAX_HASH_CHARS: usize = 64;

macro_rules! positive_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(NonZeroU64);

        impl $name {
            /// Wraps an identifier, or returns `None` for zero.
            #[must_use]
            pub const fn new(id: u64) -> Option<Self> {
                match NonZeroU64::new(id) {
                    Some(id) => Some(Self(id)),
                    None => None,
                }
            }

            /// Returns the identifier.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0.get()
            }
        }
    };
}

positive_id! {
    /// A `MediaWiki` page identifier. Unique within one site.
    PageId
}

positive_id! {
    /// A `MediaWiki` revision identifier. Unique within one site.
    RevisionId
}

/// A page title or redirect target, as the wiki wrote it.
///
/// Non-empty, at most 1024 bytes and free of control characters. Underscores,
/// capitalisation and any `#fragment` are kept exactly as exported; this type
/// does not canonicalise.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageTitle(String);

impl PageTitle {
    /// Wraps a title, or returns `None` if it is blank, too long or contains a
    /// control character.
    #[must_use]
    pub fn new(title: String) -> Option<Self> {
        is_plain_name(&title).then_some(Self(title))
    }

    /// Returns the title text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The hash the export reported for a revision's text.
///
/// `MediaWiki` writes a SHA-1 as lower-case base 36 (older versions used hex).
/// This type keeps it as an opaque identifier and checks only its shape: one
/// to 64 lower-case ASCII letters or digits. It is **not** recomputed from the
/// text, so a match with the text is not guaranteed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExportedHash(String);

impl ExportedHash {
    /// Wraps a hash, or returns `None` if it is not one to 64 lower-case ASCII
    /// letters or digits.
    #[must_use]
    pub fn new(hash: String) -> Option<Self> {
        let well_formed = (1..=MAX_HASH_CHARS).contains(&hash.len())
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase());
        well_formed.then_some(Self(hash))
    }

    /// Returns the hash text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which part of a site's identity was missing or invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceField {
    /// The human-readable site name.
    SiteName,
    /// The database name, which is the stable identifier of the wiki.
    DbName,
    /// The base URL.
    BaseUrl,
}

impl fmt::Display for SourceField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SiteName => "site name",
            Self::DbName => "database name",
            Self::BaseUrl => "base URL",
        })
    }
}

/// Which wiki an export came from, as its `siteinfo` header states.
///
/// This is provenance for attribution and source links, not a trust decision:
/// anyone can write any site name into a file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId {
    site_name: String,
    db_name: Option<String>,
    base_url: String,
}

impl SourceId {
    /// Builds a site identity.
    ///
    /// The base URL must start with `http://` or `https://` and contain no
    /// whitespace or control characters. Names must be non-blank, at most 1024
    /// bytes and free of control characters.
    ///
    /// # Errors
    ///
    /// Returns the first invalid field, checked in the order site name,
    /// database name, base URL.
    pub fn new(
        site_name: String,
        db_name: Option<String>,
        base_url: String,
    ) -> Result<Self, SourceField> {
        if !is_plain_name(&site_name) {
            return Err(SourceField::SiteName);
        }
        if db_name.as_deref().is_some_and(|name| !is_plain_name(name)) {
            return Err(SourceField::DbName);
        }
        if !is_web_url(&base_url) {
            return Err(SourceField::BaseUrl);
        }
        Ok(Self {
            site_name,
            db_name,
            base_url,
        })
    }

    /// The site's display name.
    #[must_use]
    pub fn site_name(&self) -> &str {
        &self.site_name
    }

    /// The site's database name, if the export gave one.
    #[must_use]
    pub fn db_name(&self) -> Option<&str> {
        self.db_name.as_deref()
    }

    /// The `base` URL from the export header, which is the wiki's main page.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

fn is_plain_name(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_NAME_BYTES && !text.chars().any(char::is_control)
}

fn is_web_url(text: &str) -> bool {
    let has_web_scheme = text.starts_with("https://") || text.starts_with("http://");
    has_web_scheme
        && text.len() <= MAX_URL_BYTES
        && !text.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The current revision of one page, as exported.
///
/// Contributor names, ids, edit comments and edit flags are discarded by the
/// importer: nothing downstream needs them, and attribution is by page title
/// and revision. The page's site is the [`SourceId`] of the reader that
/// produced it.
#[derive(Clone, PartialEq, Eq)]
pub struct ImportedPage {
    /// The page identifier.
    pub id: PageId,
    /// The numeric `MediaWiki` namespace (0 is the main namespace).
    pub namespace: u32,
    /// The page title.
    pub title: PageTitle,
    /// The identifier of the exported revision.
    pub revision: RevisionId,
    /// When the revision was saved.
    pub timestamp: RevisionTimestamp,
    /// The hash the export reported for the text, if any.
    pub hash: Option<ExportedHash>,
    /// The redirect target the export declared, if this page is a redirect.
    ///
    /// Recorded as written; the target page need not be in the export.
    pub redirect_target: Option<PageTitle>,
    /// The raw wikitext: unexpanded, unrendered and untrusted.
    pub text: String,
}

impl fmt::Debug for ImportedPage {
    /// Omits the wikitext, so that logging a page or failing an assertion does
    /// not print article text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImportedPage")
            .field("id", &self.id)
            .field("namespace", &self.namespace)
            .field("title", &self.title)
            .field("revision", &self.revision)
            .field("timestamp", &self.timestamp)
            .field("hash", &self.hash)
            .field("redirect_target", &self.redirect_target)
            .field("text_bytes", &self.text.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_reject_zero() {
        assert_eq!(PageId::new(0), None);
        assert_eq!(RevisionId::new(0), None);
        assert_eq!(PageId::new(7).map(PageId::get), Some(7));
        assert_eq!(
            RevisionId::new(u64::MAX).map(RevisionId::get),
            Some(u64::MAX)
        );
    }

    #[test]
    fn titles_keep_their_text_exactly() {
        let title = PageTitle::new("Cooking_guide/Experience #Table".to_owned()).unwrap();
        assert_eq!(title.as_str(), "Cooking_guide/Experience #Table");
        assert!(PageTitle::new("Café ☕".to_owned()).is_some());
    }

    #[test]
    fn titles_reject_blank_oversized_and_control_characters() {
        assert_eq!(PageTitle::new(String::new()), None);
        assert_eq!(PageTitle::new(" \t ".to_owned()), None);
        assert_eq!(PageTitle::new("Bad\ntitle".to_owned()), None);
        assert_eq!(PageTitle::new("Bad\u{0}title".to_owned()), None);
        assert!(PageTitle::new("a".repeat(MAX_NAME_BYTES)).is_some());
        assert_eq!(PageTitle::new("a".repeat(MAX_NAME_BYTES + 1)), None);
    }

    #[test]
    fn hashes_accept_base36_and_hex_but_nothing_else() {
        assert!(ExportedHash::new("phoiac9h4m842xq45sp7s6u21eteeq1".to_owned()).is_some());
        assert!(ExportedHash::new("da39a3ee5e6b4b0d3255bfef95601890afd80709".to_owned()).is_some());
        assert!(ExportedHash::new("a".repeat(MAX_HASH_CHARS)).is_some());
        for hash in ["", "ABC", "ab cd", "ab-cd", "é"] {
            assert_eq!(ExportedHash::new(hash.to_owned()), None, "{hash:?}");
        }
        assert_eq!(ExportedHash::new("a".repeat(MAX_HASH_CHARS + 1)), None);
    }

    fn source(site: &str, db: Option<&str>, url: &str) -> Result<SourceId, SourceField> {
        SourceId::new(site.to_owned(), db.map(str::to_owned), url.to_owned())
    }

    #[test]
    fn source_keeps_its_fields() {
        let source = source(
            "Example Wiki",
            Some("examplewiki"),
            "https://wiki.example/w/Main",
        )
        .unwrap();
        assert_eq!(source.site_name(), "Example Wiki");
        assert_eq!(source.db_name(), Some("examplewiki"));
        assert_eq!(source.base_url(), "https://wiki.example/w/Main");
    }

    #[test]
    fn source_reports_the_first_invalid_field() {
        let url = "https://wiki.example/";
        assert_eq!(source("", None, url), Err(SourceField::SiteName));
        assert_eq!(source("W", Some("a\nb"), url), Err(SourceField::DbName));
        assert_eq!(
            source("W", None, "ftp://wiki.example/"),
            Err(SourceField::BaseUrl)
        );
        assert_eq!(
            source("W", None, "https://wiki .example/"),
            Err(SourceField::BaseUrl)
        );
        assert_eq!(source("", Some(""), "x"), Err(SourceField::SiteName));
        assert!(
            source(
                "W",
                None,
                &format!("https://h/{}", "a".repeat(MAX_URL_BYTES))
            )
            .is_err()
        );
    }

    #[test]
    fn debug_output_omits_the_wikitext() {
        let page = ImportedPage {
            id: PageId::new(1).unwrap(),
            namespace: 0,
            title: PageTitle::new("Title".to_owned()).unwrap(),
            revision: RevisionId::new(2).unwrap(),
            timestamp: RevisionTimestamp::parse("2026-01-01T00:00:00Z").unwrap(),
            hash: None,
            redirect_target: None,
            text: "secret article text".to_owned(),
        };
        let debug = format!("{page:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("text_bytes: 19"));
    }
}
