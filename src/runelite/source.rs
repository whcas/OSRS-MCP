//! Reading `character.json` from a user-configured local directory.
//!
//! This is the only place in the `runelite` module that touches the file
//! system. It reads one fixed file name from one configured directory, with a
//! byte limit, and hands the bytes to [`parse_character_export`]. Callers
//! cannot supply a path, and nothing here discovers, scans, watches or caches.
//!
//! Errors and `Debug` output never contain the configured path or any file
//! content: both are personal. See `docs/research/runelite-export-formats.md`
//! for why a single read can see a half-written file.

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::{CharacterExport, CharacterParseError, parse_character_export};

/// Environment variable naming the account's Character Exporter directory,
/// the one that contains `character.json`.
pub const EXPORT_DIR_VAR: &str = "OSRS_MCP_RUNELITE_EXPORT_DIR";

/// The only file this module reads.
pub const CHARACTER_FILE_NAME: &str = "character.json";

/// Largest `character.json` that is read, in bytes.
///
/// A real file is a few kilobytes (one small object per skill). The limit is
/// generous but keeps a wrong or hostile file from being read into memory.
pub const MAX_CHARACTER_EXPORT_BYTES: u64 = 256 * 1024;

/// Where `character.json` is read from, if anywhere.
#[derive(Clone)]
pub struct LocalExportSource {
    directory: Option<PathBuf>,
}

/// Prints only whether a directory is configured: the path is personal.
impl fmt::Debug for LocalExportSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalExportSource")
            .field("configured", &self.directory.is_some())
            .finish()
    }
}

impl LocalExportSource {
    /// A source that reads from `directory`. The path is not checked until a
    /// read, so an invalid path does not stop the server from starting.
    #[must_use]
    pub const fn new(directory: PathBuf) -> Self {
        Self {
            directory: Some(directory),
        }
    }

    /// A source with nothing configured: every read reports
    /// [`ExportReadError::NotConfigured`].
    #[must_use]
    pub const fn unconfigured() -> Self {
        Self { directory: None }
    }

    /// Builds the source from the value of [`EXPORT_DIR_VAR`]. An unset or
    /// empty value means not configured.
    #[must_use]
    pub fn from_env_value(value: Option<OsString>) -> Self {
        match value {
            Some(path) if !path.is_empty() => Self::new(PathBuf::from(path)),
            _ => Self::unconfigured(),
        }
    }

    /// Builds the source from the process environment ([`EXPORT_DIR_VAR`]).
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_env_value(std::env::var_os(EXPORT_DIR_VAR))
    }

    /// Whether a directory has been configured.
    #[must_use]
    pub const fn is_configured(&self) -> bool {
        self.directory.is_some()
    }

    /// Reads and parses `character.json` from the configured directory.
    ///
    /// The read is blocking, happens once per call and is not retried:
    /// The `RuneLite` plugin does not write atomically, so a read can land in the
    /// middle of a write. That surfaces as a transient parse error (see
    /// [`ExportReadError::is_transient`]) and the caller decides whether to
    /// try again. Nothing is cached, so each call reflects the file now.
    ///
    /// # Errors
    ///
    /// Returns [`ExportReadError`] if nothing is configured, the file cannot
    /// be read within [`MAX_CHARACTER_EXPORT_BYTES`], or its content is not a
    /// valid export.
    pub fn read_character(&self) -> Result<CharacterExport, ExportReadError> {
        let directory = self
            .directory
            .as_deref()
            .ok_or(ExportReadError::NotConfigured)?;
        let bytes = read_bounded(&directory.join(CHARACTER_FILE_NAME), directory)?;
        Ok(parse_character_export(&bytes)?)
    }
}

/// Why `character.json` could not be read and imported.
///
/// No variant carries a path, file content or operating-system message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExportReadError {
    /// No export directory is configured.
    #[error("no RuneLite export directory is configured")]
    NotConfigured,
    /// The configured directory does not exist or is not a directory.
    #[error("the configured export directory does not exist or is not a directory")]
    DirectoryUnavailable,
    /// The directory exists but has no `character.json`.
    #[error("the export directory contains no character.json")]
    FileNotFound,
    /// `character.json` exists but is not a regular file.
    #[error("character.json is not a regular file")]
    NotARegularFile,
    /// The operating system refused access.
    #[error("permission denied reading character.json")]
    PermissionDenied,
    /// The file is larger than the read limit.
    #[error("character.json is larger than the {limit} byte limit")]
    TooLarge {
        /// The limit, in bytes.
        limit: u64,
    },
    /// Any other read failure, identified by its kind only.
    #[error("character.json could not be read ({kind})")]
    Io {
        /// The error kind reported by the operating system.
        kind: io::ErrorKind,
    },
    /// The file was read but is not an acceptable export.
    #[error("character.json could not be imported")]
    Parse(#[from] CharacterParseError),
}

impl ExportReadError {
    /// Whether reading again shortly may succeed, because the file was
    /// probably caught mid-write.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(self, Self::Parse(error) if error.is_transient())
    }
}

/// Reads `path` if it is a regular file of at most
/// [`MAX_CHARACTER_EXPORT_BYTES`] bytes.
fn read_bounded(path: &Path, directory: &Path) -> Result<Vec<u8>, ExportReadError> {
    // Checked before opening: opening a named pipe would block until a writer
    // appears. `metadata` follows symlinks, as the configured directory is
    // the user's own.
    let metadata = fs::metadata(path).map_err(|error| classify_missing(&error, directory))?;
    if !metadata.is_file() {
        return Err(ExportReadError::NotARegularFile);
    }

    let file = File::open(path).map_err(|error| classify_io(&error))?;
    let mut bytes = Vec::new();
    // One byte past the limit tells "exactly at the limit" from "larger",
    // even if the file grows after the check above.
    file.take(MAX_CHARACTER_EXPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| classify_io(&error))?;
    if u64::try_from(bytes.len()).map_or(true, |read| read > MAX_CHARACTER_EXPORT_BYTES) {
        return Err(ExportReadError::TooLarge {
            limit: MAX_CHARACTER_EXPORT_BYTES,
        });
    }
    Ok(bytes)
}

/// Classifies a failure to look up the file, separating a missing directory
/// from a missing file.
fn classify_missing(error: &io::Error, directory: &Path) -> ExportReadError {
    match error.kind() {
        io::ErrorKind::NotFound if directory_exists(directory) => ExportReadError::FileNotFound,
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
            ExportReadError::DirectoryUnavailable
        }
        _ => classify_io(error),
    }
}

fn directory_exists(directory: &Path) -> bool {
    fs::metadata(directory).is_ok_and(|metadata| metadata.is_dir())
}

fn classify_io(error: &io::Error) -> ExportReadError {
    match error.kind() {
        io::ErrorKind::PermissionDenied => ExportReadError::PermissionDenied,
        kind => ExportReadError::Io { kind },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    const SYNTHETIC_EXPORT: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/character.synthetic.json");

    /// A fresh directory under the system temp directory, removed on drop.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "osrs-mcp-source-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write_character(&self, content: &[u8]) {
            fs::write(self.0.join(CHARACTER_FILE_NAME), content).unwrap();
        }

        fn source(&self) -> LocalExportSource {
            LocalExportSource::new(self.0.clone())
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn an_unset_or_empty_variable_means_not_configured() {
        for value in [None, Some(OsString::new())] {
            let source = LocalExportSource::from_env_value(value);
            assert!(!source.is_configured());
            assert_eq!(source.read_character(), Err(ExportReadError::NotConfigured));
        }
    }

    #[test]
    fn a_non_empty_variable_configures_the_source() {
        let source = LocalExportSource::from_env_value(Some(OsString::from("some-dir")));
        assert!(source.is_configured());
    }

    #[test]
    fn debug_output_does_not_contain_the_path() {
        let source = LocalExportSource::new(PathBuf::from("private-directory-name"));
        let text = format!("{source:?}");
        assert!(!text.contains("private-directory-name"), "{text}");
        assert!(text.contains("configured: true"), "{text}");
    }

    #[test]
    fn a_valid_export_is_read_and_parsed() {
        let dir = TestDir::new();
        dir.write_character(SYNTHETIC_EXPORT);

        let export = dir.source().read_character().unwrap();

        assert_eq!(export, parse_character_export(SYNTHETIC_EXPORT).unwrap());
    }

    #[test]
    fn parser_warnings_are_preserved() {
        // Level 5 with 0 experience disagrees with the experience table.
        let dir = TestDir::new();
        dir.write_character(
            br#"{"exported_at":"2026-01-01T12:00:00Z","stats":{"Attack":{"real_level":5,"experience":0}}}"#,
        );

        let export = dir.source().read_character().unwrap();

        assert_eq!(export.warnings().len(), 1);
    }

    #[test]
    fn a_missing_directory_is_reported_as_such() {
        let dir = TestDir::new();
        let source = LocalExportSource::new(dir.0.join("does-not-exist"));

        assert_eq!(
            source.read_character(),
            Err(ExportReadError::DirectoryUnavailable)
        );
    }

    #[test]
    fn a_configured_path_that_is_a_file_is_an_unavailable_directory() {
        let dir = TestDir::new();
        dir.write_character(SYNTHETIC_EXPORT);
        let source = LocalExportSource::new(dir.0.join(CHARACTER_FILE_NAME));

        assert_eq!(
            source.read_character(),
            Err(ExportReadError::DirectoryUnavailable)
        );
    }

    #[test]
    fn a_directory_without_the_file_is_reported_as_such() {
        let dir = TestDir::new();

        assert_eq!(
            dir.source().read_character(),
            Err(ExportReadError::FileNotFound)
        );
    }

    #[test]
    fn other_files_in_the_directory_are_never_read() {
        let dir = TestDir::new();
        fs::write(dir.0.join("bank.json"), SYNTHETIC_EXPORT).unwrap();

        assert_eq!(
            dir.source().read_character(),
            Err(ExportReadError::FileNotFound)
        );
    }

    #[test]
    fn a_directory_named_like_the_file_is_not_read() {
        let dir = TestDir::new();
        fs::create_dir(dir.0.join(CHARACTER_FILE_NAME)).unwrap();

        assert_eq!(
            dir.source().read_character(),
            Err(ExportReadError::NotARegularFile)
        );
    }

    #[test]
    fn a_file_over_the_limit_is_rejected_without_being_parsed() {
        let dir = TestDir::new();
        let oversized = vec![b' '; usize::try_from(MAX_CHARACTER_EXPORT_BYTES).unwrap() + 1];
        dir.write_character(&oversized);

        assert_eq!(
            dir.source().read_character(),
            Err(ExportReadError::TooLarge {
                limit: MAX_CHARACTER_EXPORT_BYTES
            })
        );
    }

    #[test]
    fn a_file_exactly_at_the_limit_is_read() {
        // Whitespace padding is valid JSON, so the only thing under test is
        // that the limit itself is not treated as too large.
        let dir = TestDir::new();
        let mut content = SYNTHETIC_EXPORT.to_vec();
        content.resize(usize::try_from(MAX_CHARACTER_EXPORT_BYTES).unwrap(), b' ');
        dir.write_character(&content);

        assert!(dir.source().read_character().is_ok());
    }

    #[test]
    fn a_truncated_file_is_a_transient_parse_error() {
        let dir = TestDir::new();
        dir.write_character(&SYNTHETIC_EXPORT[..SYNTHETIC_EXPORT.len() / 2]);

        let error = dir.source().read_character().unwrap_err();

        assert_eq!(
            error,
            ExportReadError::Parse(CharacterParseError::Truncated)
        );
        assert!(error.is_transient());
    }

    #[test]
    fn an_empty_file_is_a_transient_parse_error() {
        let dir = TestDir::new();
        dir.write_character(b"");

        let error = dir.source().read_character().unwrap_err();

        assert!(error.is_transient());
    }

    #[test]
    fn invalid_json_is_a_parse_error_that_is_not_transient() {
        let dir = TestDir::new();
        dir.write_character(b"this is not json");

        let error = dir.source().read_character().unwrap_err();

        assert_eq!(
            error,
            ExportReadError::Parse(CharacterParseError::InvalidJson)
        );
        assert!(!error.is_transient());
    }

    #[test]
    fn errors_other_than_parse_failures_are_not_transient() {
        assert!(!ExportReadError::FileNotFound.is_transient());
        assert!(!ExportReadError::NotConfigured.is_transient());
    }

    #[test]
    fn permission_failures_are_classified_without_the_os_message() {
        let error = io::Error::new(io::ErrorKind::PermissionDenied, "/private/path: denied");

        assert_eq!(classify_io(&error), ExportReadError::PermissionDenied);
    }

    #[test]
    fn other_io_failures_keep_only_their_kind() {
        let error = io::Error::other("/private/path: something broke");

        let classified = classify_io(&error);

        assert_eq!(
            classified,
            ExportReadError::Io {
                kind: io::ErrorKind::Other
            }
        );
        assert!(!classified.to_string().contains("private"));
    }

    #[test]
    fn error_messages_do_not_contain_the_configured_path() {
        let dir = TestDir::new();
        let source = LocalExportSource::new(dir.0.join("missing-private-name"));
        let directory_name = dir.0.file_name().unwrap().to_string_lossy().into_owned();

        let message = source.read_character().unwrap_err().to_string();

        assert!(!message.contains("missing-private-name"), "{message}");
        assert!(!message.contains(&directory_name), "{message}");
    }
}
