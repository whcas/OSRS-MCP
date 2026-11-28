//! Reading Character Exporter files from a user-configured local directory.
//!
//! This is the only place in the `runelite` module that touches the file
//! system. It reads fixed file names from one configured directory, each with
//! its own byte limit, and hands the bytes to the matching parser. Callers
//! cannot supply a path, and nothing here discovers, scans, watches or caches.
//! Each read is independent: files are written separately, so results from
//! different calls are separate observations, never one consistent snapshot.
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

use super::{
    CharacterExport, CharacterParseError, ContainerKind, ContainerParseError, ContainerSnapshot,
    MAX_CONTAINER_EXPORT_BYTES, MAX_QUEST_EXPORT_BYTES, QuestParseError, QuestProgress,
    parse_character_export, parse_container_export, parse_quest_export,
};

/// Environment variable naming the account's Character Exporter directory,
/// the one that contains `character.json`.
pub const EXPORT_DIR_VAR: &str = "OSRS_MCP_RUNELITE_EXPORT_DIR";

/// The only file this module reads.
pub const CHARACTER_FILE_NAME: &str = "character.json";

/// Bank holdings file.
pub const BANK_FILE_NAME: &str = "bank.json";

/// Inventory file.
pub const INVENTORY_FILE_NAME: &str = "inventory.json";

/// Equipment file.
pub const EQUIPMENT_FILE_NAME: &str = "equipment.json";

/// Quest progress file.
pub const QUEST_FILE_NAME: &str = "quests.json";

/// The only file this module reads for `kind`.
#[must_use]
pub const fn container_file_name(kind: ContainerKind) -> &'static str {
    match kind {
        ContainerKind::Bank => BANK_FILE_NAME,
        ContainerKind::Inventory => INVENTORY_FILE_NAME,
        ContainerKind::Equipment => EQUIPMENT_FILE_NAME,
    }
}

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
    fn from_env_value(value: Option<OsString>) -> Self {
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
        self.read_parsed(
            CHARACTER_FILE_NAME,
            MAX_CHARACTER_EXPORT_BYTES,
            parse_character_export,
        )
    }

    /// Reads and parses the bank, inventory or equipment file.
    ///
    /// Reads and failures behave as for [`read_character`](Self::read_character),
    /// with the limit [`MAX_CONTAINER_EXPORT_BYTES`]. A missing file is
    /// [`ExportReadError::FileNotFound`], never an empty container: the
    /// exporter writes a container only after it has been observed, and an
    /// old file may remain from an earlier session. An observed empty
    /// container is a valid snapshot with no rows. Each container file has
    /// its own timestamp, so results for different kinds are separate
    /// observations.
    ///
    /// # Errors
    ///
    /// Returns [`ExportReadError`] if nothing is configured, the file cannot
    /// be read within the limit, or its content is not a valid export of the
    /// requested kind.
    pub fn read_container(
        &self,
        kind: ContainerKind,
    ) -> Result<ContainerSnapshot, ExportReadError> {
        self.read_parsed(container_file_name(kind), container_limit(), |bytes| {
            parse_container_export(kind, bytes)
        })
    }

    /// Reads and parses `quests.json`.
    ///
    /// Reads and failures behave as for [`read_character`](Self::read_character),
    /// with the limit [`MAX_QUEST_EXPORT_BYTES`]. A missing file is
    /// [`ExportReadError::FileNotFound`]; it never means that no quest is
    /// started, and a quest absent from a valid file is unrecorded rather than
    /// not started. The file has its own `exported_at`.
    ///
    /// # Errors
    ///
    /// Returns [`ExportReadError`] if nothing is configured, the file cannot
    /// be read within the limit, or its content is not a valid export.
    pub fn read_quests(&self) -> Result<QuestProgress, ExportReadError> {
        self.read_parsed(QUEST_FILE_NAME, quest_limit(), parse_quest_export)
    }

    /// Reads `file_name` within `limit` bytes and parses it, so that every
    /// dataset reports read and parse failures the same way.
    fn read_parsed<T, E>(
        &self,
        file_name: &str,
        limit: u64,
        parse: impl FnOnce(&[u8]) -> Result<T, E>,
    ) -> Result<T, ExportReadError>
    where
        ParseFailure: From<E>,
    {
        let bytes = self.read_file(file_name, limit)?;
        parse(&bytes).map_err(|error| ExportReadError::Parse(ParseFailure::from(error)))
    }

    fn read_file(&self, file_name: &str, limit: u64) -> Result<Vec<u8>, ExportReadError> {
        let directory = self
            .directory
            .as_deref()
            .ok_or(ExportReadError::NotConfigured)?;
        read_bounded(&directory.join(file_name), directory, limit)
    }
}

/// The container parser's byte limit as the reader's unit.
fn container_limit() -> u64 {
    u64::try_from(MAX_CONTAINER_EXPORT_BYTES).unwrap_or(u64::MAX)
}

/// The quest parser's byte limit as the reader's unit.
fn quest_limit() -> u64 {
    u64::try_from(MAX_QUEST_EXPORT_BYTES).unwrap_or(u64::MAX)
}

/// Why an export file could not be read and imported.
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
    /// The directory exists but has no file for the requested dataset.
    #[error("the export directory contains no file for this dataset")]
    FileNotFound,
    /// The dataset's file exists but is not a regular file.
    #[error("the export file is not a regular file")]
    NotARegularFile,
    /// The operating system refused access.
    #[error("permission denied reading the export file")]
    PermissionDenied,
    /// The file is larger than the read limit.
    #[error("the export file is larger than the {limit} byte limit")]
    TooLarge {
        /// The limit, in bytes.
        limit: u64,
    },
    /// Any other read failure, identified by its kind only.
    #[error("the export file could not be read ({kind})")]
    Io {
        /// The error kind reported by the operating system.
        kind: io::ErrorKind,
    },
    /// The file was read but is not an acceptable export.
    #[error("the export file could not be imported")]
    Parse(#[source] ParseFailure),
}

impl ExportReadError {
    /// Whether reading again shortly may succeed, because the file was
    /// probably caught mid-write.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(self, Self::Parse(failure) if failure.is_transient())
    }
}

/// The parser error behind [`ExportReadError::Parse`], by dataset. The text
/// is the parser's own sanitised description.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseFailure {
    /// `character.json` was rejected.
    #[error(transparent)]
    Character(#[from] CharacterParseError),
    /// A bank, inventory or equipment file was rejected.
    #[error(transparent)]
    Container(#[from] ContainerParseError),
    /// `quests.json` was rejected.
    #[error(transparent)]
    Quest(#[from] QuestParseError),
}

impl ParseFailure {
    /// Whether the input ended unexpectedly, as it can during a write.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        match self {
            Self::Character(error) => error.is_transient(),
            Self::Container(error) => error.is_transient(),
            Self::Quest(error) => error.is_transient(),
        }
    }
}

/// Reads `path` if it is a regular file of at most `limit` bytes.
fn read_bounded(path: &Path, directory: &Path, limit: u64) -> Result<Vec<u8>, ExportReadError> {
    // Checked before opening: opening a named pipe would block until a writer
    // appears. `metadata` follows symlinks. This is a guard against a
    // mistaken file, not a defence against someone replacing the file between
    // this check and the open: the directory is the user's own and is trusted
    // not to be modified adversarially.
    let metadata = fs::metadata(path).map_err(|error| classify_missing(&error, directory))?;
    if !metadata.is_file() {
        return Err(ExportReadError::NotARegularFile);
    }

    let file = File::open(path).map_err(|error| classify_io(&error))?;
    let mut bytes = Vec::new();
    // One byte past the limit tells "exactly at the limit" from "larger",
    // even if the file grows after the check above.
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| classify_io(&error))?;
    if u64::try_from(bytes.len()).map_or(true, |read| read > limit) {
        return Err(ExportReadError::TooLarge { limit });
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
    use tempfile::TempDir;

    use super::*;

    const SYNTHETIC_EXPORT: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/character.synthetic.json");
    const SYNTHETIC_BANK: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/bank.synthetic.json");
    const SYNTHETIC_INVENTORY: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/inventory.synthetic.json");
    const SYNTHETIC_EQUIPMENT: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/equipment.synthetic.json");

    fn new_dir() -> TempDir {
        TempDir::with_prefix("osrs-mcp-source-").unwrap()
    }

    fn write_character(dir: &TempDir, content: &[u8]) {
        fs::write(dir.path().join(CHARACTER_FILE_NAME), content).unwrap();
    }

    fn source_for(dir: &TempDir) -> LocalExportSource {
        LocalExportSource::new(dir.path().to_path_buf())
    }

    #[test]
    fn an_unset_or_empty_variable_means_not_configured() {
        for value in [None, Some(OsString::new())] {
            let source = LocalExportSource::from_env_value(value);
            assert_eq!(source.read_character(), Err(ExportReadError::NotConfigured));
        }
    }

    #[test]
    fn a_non_empty_variable_names_the_directory_to_read() {
        let dir = new_dir();
        write_character(&dir, SYNTHETIC_EXPORT);

        let source = LocalExportSource::from_env_value(Some(dir.path().as_os_str().to_owned()));

        assert!(source.read_character().is_ok());
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
        let dir = new_dir();
        write_character(&dir, SYNTHETIC_EXPORT);

        let export = source_for(&dir).read_character().unwrap();

        assert_eq!(export, parse_character_export(SYNTHETIC_EXPORT).unwrap());
    }

    #[test]
    fn parser_warnings_are_preserved() {
        // Level 5 with 0 experience disagrees with the experience table.
        let dir = new_dir();
        write_character(&dir,
            br#"{"exported_at":"2026-01-01T12:00:00Z","stats":{"Attack":{"real_level":5,"experience":0}}}"#,
        );

        let export = source_for(&dir).read_character().unwrap();

        assert_eq!(export.warnings().len(), 1);
    }

    #[test]
    fn a_missing_directory_is_reported_as_such() {
        let dir = new_dir();
        let source = LocalExportSource::new(dir.path().join("does-not-exist"));

        assert_eq!(
            source.read_character(),
            Err(ExportReadError::DirectoryUnavailable)
        );
    }

    #[test]
    fn a_configured_path_that_is_a_file_is_an_unavailable_directory() {
        let dir = new_dir();
        write_character(&dir, SYNTHETIC_EXPORT);
        let source = LocalExportSource::new(dir.path().join(CHARACTER_FILE_NAME));

        assert_eq!(
            source.read_character(),
            Err(ExportReadError::DirectoryUnavailable)
        );
    }

    #[test]
    fn a_directory_without_the_file_is_reported_as_such() {
        let dir = new_dir();

        assert_eq!(
            source_for(&dir).read_character(),
            Err(ExportReadError::FileNotFound)
        );
    }

    #[test]
    fn other_files_in_the_directory_are_never_read() {
        let dir = new_dir();
        fs::write(dir.path().join("bank.json"), SYNTHETIC_EXPORT).unwrap();

        assert_eq!(
            source_for(&dir).read_character(),
            Err(ExportReadError::FileNotFound)
        );
    }

    #[test]
    fn a_directory_named_like_the_file_is_not_read() {
        let dir = new_dir();
        fs::create_dir(dir.path().join(CHARACTER_FILE_NAME)).unwrap();

        assert_eq!(
            source_for(&dir).read_character(),
            Err(ExportReadError::NotARegularFile)
        );
    }

    #[test]
    fn a_file_over_the_limit_is_rejected_without_being_parsed() {
        let dir = new_dir();
        let oversized = vec![b' '; usize::try_from(MAX_CHARACTER_EXPORT_BYTES).unwrap() + 1];
        write_character(&dir, &oversized);

        assert_eq!(
            source_for(&dir).read_character(),
            Err(ExportReadError::TooLarge {
                limit: MAX_CHARACTER_EXPORT_BYTES
            })
        );
    }

    #[test]
    fn a_file_exactly_at_the_limit_is_read() {
        // Whitespace padding is valid JSON, so the only thing under test is
        // that the limit itself is not treated as too large.
        let dir = new_dir();
        let mut content = SYNTHETIC_EXPORT.to_vec();
        content.resize(usize::try_from(MAX_CHARACTER_EXPORT_BYTES).unwrap(), b' ');
        write_character(&dir, &content);

        assert!(source_for(&dir).read_character().is_ok());
    }

    #[test]
    fn a_truncated_file_is_a_transient_parse_error() {
        let dir = new_dir();
        write_character(&dir, &SYNTHETIC_EXPORT[..SYNTHETIC_EXPORT.len() / 2]);

        let error = source_for(&dir).read_character().unwrap_err();

        assert_eq!(
            error,
            ExportReadError::Parse(CharacterParseError::Truncated.into())
        );
        assert!(error.is_transient());
    }

    #[test]
    fn an_empty_file_is_a_transient_parse_error() {
        let dir = new_dir();
        write_character(&dir, b"");

        let error = source_for(&dir).read_character().unwrap_err();

        assert!(error.is_transient());
    }

    #[test]
    fn invalid_json_is_a_parse_error_that_is_not_transient() {
        let dir = new_dir();
        write_character(&dir, b"this is not json");

        let error = source_for(&dir).read_character().unwrap_err();

        assert_eq!(
            error,
            ExportReadError::Parse(CharacterParseError::InvalidJson.into())
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
        let dir = new_dir();
        let source = LocalExportSource::new(dir.path().join("missing-private-name"));
        let directory_name = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let message = source.read_character().unwrap_err().to_string();

        assert!(!message.contains("missing-private-name"), "{message}");
        assert!(!message.contains(&directory_name), "{message}");
    }

    fn write_container(dir: &TempDir, kind: ContainerKind, content: &[u8]) {
        fs::write(dir.path().join(container_file_name(kind)), content).unwrap();
    }

    fn synthetic_container(kind: ContainerKind) -> &'static [u8] {
        match kind {
            ContainerKind::Bank => SYNTHETIC_BANK,
            ContainerKind::Inventory => SYNTHETIC_INVENTORY,
            ContainerKind::Equipment => SYNTHETIC_EQUIPMENT,
        }
    }

    const SYNTHETIC_QUESTS: &[u8] =
        include_bytes!("../../tests/fixtures/runelite_export/quests.synthetic.json");

    const ALL_KINDS: [ContainerKind; 3] = [
        ContainerKind::Bank,
        ContainerKind::Inventory,
        ContainerKind::Equipment,
    ];

    #[test]
    fn each_container_kind_reads_only_its_own_fixed_file() {
        for kind in ALL_KINDS {
            let dir = new_dir();
            write_container(&dir, kind, synthetic_container(kind));

            let snapshot = source_for(&dir).read_container(kind).unwrap();

            assert_eq!(
                snapshot,
                parse_container_export(kind, synthetic_container(kind)).unwrap()
            );
            for other in ALL_KINDS.into_iter().filter(|other| *other != kind) {
                assert_eq!(
                    source_for(&dir).read_container(other),
                    Err(ExportReadError::FileNotFound),
                    "{other:?} must not read the {kind:?} file"
                );
            }
        }
    }

    #[test]
    fn a_missing_container_file_is_not_an_empty_container() {
        let dir = new_dir();

        for kind in ALL_KINDS {
            assert_eq!(
                source_for(&dir).read_container(kind),
                Err(ExportReadError::FileNotFound)
            );
        }
    }

    #[test]
    fn an_observed_empty_container_is_a_valid_snapshot() {
        let dir = new_dir();
        write_container(&dir, ContainerKind::Equipment, SYNTHETIC_EQUIPMENT);

        let snapshot = source_for(&dir)
            .read_container(ContainerKind::Equipment)
            .unwrap();

        assert_eq!(snapshot.rows(), []);
        assert_eq!(snapshot.warnings(), []);
    }

    #[test]
    fn a_file_holding_another_container_kind_is_rejected() {
        let dir = new_dir();
        write_container(&dir, ContainerKind::Bank, SYNTHETIC_EQUIPMENT);

        assert_eq!(
            source_for(&dir).read_container(ContainerKind::Bank),
            Err(ExportReadError::Parse(
                ContainerParseError::WrongKind.into()
            ))
        );
    }

    #[test]
    fn container_files_keep_independent_timestamps() {
        let dir = new_dir();
        write_container(&dir, ContainerKind::Bank, SYNTHETIC_BANK);
        let newer = String::from_utf8(SYNTHETIC_INVENTORY.to_vec())
            .unwrap()
            .replace("2026-01-01T12:00Z", "2026-02-02T09:30Z");
        write_container(&dir, ContainerKind::Inventory, newer.as_bytes());

        let bank = source_for(&dir)
            .read_container(ContainerKind::Bank)
            .unwrap();
        let inventory = source_for(&dir)
            .read_container(ContainerKind::Inventory)
            .unwrap();

        assert_ne!(bank.exported_at(), inventory.exported_at());
    }

    #[test]
    fn container_reads_are_fresh_each_call() {
        let dir = new_dir();
        write_container(&dir, ContainerKind::Equipment, SYNTHETIC_EQUIPMENT);
        let source = source_for(&dir);
        let first = source.read_container(ContainerKind::Equipment).unwrap();

        let populated = String::from_utf8(SYNTHETIC_INVENTORY.to_vec())
            .unwrap()
            .replace("\"inventory\"", "\"equipment\"");
        write_container(&dir, ContainerKind::Equipment, populated.as_bytes());
        let second = source.read_container(ContainerKind::Equipment).unwrap();

        assert_ne!(first.rows(), second.rows());
    }

    #[test]
    fn a_container_file_over_the_parser_limit_is_rejected_without_being_parsed() {
        let dir = new_dir();
        let oversized = vec![b' '; MAX_CONTAINER_EXPORT_BYTES + 1];
        write_container(&dir, ContainerKind::Bank, &oversized);

        assert_eq!(
            source_for(&dir).read_container(ContainerKind::Bank),
            Err(ExportReadError::TooLarge {
                limit: container_limit()
            })
        );
    }

    #[test]
    fn a_container_file_exactly_at_the_limit_is_read() {
        let dir = new_dir();
        let mut content = SYNTHETIC_BANK.to_vec();
        content.resize(MAX_CONTAINER_EXPORT_BYTES, b' ');
        write_container(&dir, ContainerKind::Bank, &content);

        assert!(source_for(&dir).read_container(ContainerKind::Bank).is_ok());
    }

    #[test]
    fn the_character_limit_is_unchanged_by_the_container_limit() {
        let dir = new_dir();
        let oversized = vec![b' '; usize::try_from(MAX_CHARACTER_EXPORT_BYTES).unwrap() + 1];
        write_character(&dir, &oversized);

        assert_eq!(
            source_for(&dir).read_character(),
            Err(ExportReadError::TooLarge {
                limit: MAX_CHARACTER_EXPORT_BYTES
            })
        );
    }

    #[test]
    fn a_truncated_container_file_is_transient() {
        let dir = new_dir();
        write_container(
            &dir,
            ContainerKind::Bank,
            &SYNTHETIC_BANK[..SYNTHETIC_BANK.len() / 2],
        );

        let error = source_for(&dir)
            .read_container(ContainerKind::Bank)
            .unwrap_err();

        assert!(error.is_transient());
    }

    #[test]
    fn container_errors_do_not_contain_the_path_or_file_content() {
        let dir = new_dir();
        write_container(&dir, ContainerKind::Bank, b"{\"private-marker\": 1");
        let directory_name = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let message = format!(
            "{:?} {}",
            source_for(&dir).read_container(ContainerKind::Bank),
            source_for(&dir)
                .read_container(ContainerKind::Bank)
                .unwrap_err()
        );

        assert!(!message.contains("private-marker"), "{message}");
        assert!(!message.contains(&directory_name), "{message}");
    }

    fn write_quests(dir: &TempDir, content: &[u8]) {
        fs::write(dir.path().join(QUEST_FILE_NAME), content).unwrap();
    }

    #[test]
    fn a_valid_quest_file_is_read_and_parsed() {
        let dir = new_dir();
        write_quests(&dir, SYNTHETIC_QUESTS);

        let progress = source_for(&dir).read_quests().unwrap();

        assert_eq!(progress, parse_quest_export(SYNTHETIC_QUESTS).unwrap());
    }

    #[test]
    fn a_missing_quest_file_is_not_a_report_that_no_quest_is_started() {
        let dir = new_dir();
        write_character(&dir, SYNTHETIC_EXPORT);

        assert_eq!(
            source_for(&dir).read_quests(),
            Err(ExportReadError::FileNotFound)
        );
    }

    #[test]
    fn quests_are_read_fresh_each_call() {
        let dir = new_dir();
        write_quests(&dir, SYNTHETIC_QUESTS);
        let source = source_for(&dir);
        let first = source.read_quests().unwrap();

        let rewritten = String::from_utf8(SYNTHETIC_QUESTS.to_vec())
            .unwrap()
            .replace("2026-02-01T11:12:13.123456-02:00", "2026-03-03T08:15Z");
        write_quests(&dir, rewritten.as_bytes());
        let second = source.read_quests().unwrap();

        assert_ne!(first.exported_at(), second.exported_at());
    }

    #[test]
    fn a_quest_file_over_the_parser_limit_is_rejected_without_being_parsed() {
        let dir = new_dir();
        write_quests(&dir, &vec![b' '; MAX_QUEST_EXPORT_BYTES + 1]);

        assert_eq!(
            source_for(&dir).read_quests(),
            Err(ExportReadError::TooLarge {
                limit: quest_limit()
            })
        );
    }

    #[test]
    fn a_quest_file_exactly_at_the_limit_is_read() {
        let dir = new_dir();
        let mut content = SYNTHETIC_QUESTS.to_vec();
        content.resize(MAX_QUEST_EXPORT_BYTES, b' ');
        write_quests(&dir, &content);

        assert!(source_for(&dir).read_quests().is_ok());
    }

    #[test]
    fn a_truncated_quest_file_is_transient_and_invalid_content_is_not() {
        let dir = new_dir();
        write_quests(&dir, &SYNTHETIC_QUESTS[..SYNTHETIC_QUESTS.len() / 2]);
        assert!(source_for(&dir).read_quests().unwrap_err().is_transient());

        write_quests(&dir, b"this is not json");
        let error = source_for(&dir).read_quests().unwrap_err();

        assert_eq!(
            error,
            ExportReadError::Parse(QuestParseError::InvalidJson.into())
        );
        assert!(!error.is_transient());
    }

    #[test]
    fn quest_errors_do_not_contain_the_path_or_file_content() {
        let dir = new_dir();
        write_quests(&dir, b"{\"private-marker\": 1");
        let directory_name = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let error = source_for(&dir).read_quests().unwrap_err();
        let message = format!("{error:?} {error}");

        assert!(!message.contains("private-marker"), "{message}");
        assert!(!message.contains(&directory_name), "{message}");
    }
}
