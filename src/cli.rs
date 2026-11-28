//! Command-line grammar and output, the adapter between the terminal and the
//! library. It is public for the binary and the tests, not a stable interface.
//!
//! Parsing is separate from execution: this module turns
//! arguments into plain requests for [`osrs_mcp::wiki::commands`] and knows
//! nothing about databases, printing or exit codes, and no `clap` type leaves
//! it.

pub mod render;

use std::path::PathBuf;

use crate::wiki::commands::{Corpus, ImportRequest, PageRequest, SearchRequest};
use crate::wiki::{InvalidSearch, SearchQuery};
use clap::{Args, Parser, Subcommand};

/// The environment variable that names the Wiki database when `--db` is not
/// given. There is no default location: nothing is written unless the user
/// chooses a path.
const DATABASE_ENV: &str = "OSRS_MCP_WIKI_DB";

/// Read-only advisory MCP server for Old School `RuneScape` Hardcore worlds, and
/// local Wiki tools.
#[derive(Debug, Parser)]
#[command(name = "osrs-mcp", version, subcommand_required = true)]
pub struct Cli {
    /// The command to run.
    #[command(subcommand)]
    pub command: Command,
}

/// A top-level command.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Serve MCP over stdio (for MCP clients).
    Serve,
    /// Work with a local Wiki database built from manual exports.
    Wiki {
        /// The Wiki command to run.
        #[command(subcommand)]
        command: WikiCommand,
    },
}

/// A Wiki command. All of them are offline; only `import` writes.
#[derive(Debug, Subcommand)]
pub enum WikiCommand {
    /// Import a Special:Export XML file into the database, creating it if needed.
    ///
    /// The whole file is one transaction: if any part of it fails, nothing is
    /// stored. Importing the same file again changes nothing.
    Import(ImportArgs),
    /// Search the imported pages for words; every word must match.
    Search(SearchArgs),
    /// Show a page by title, following redirects.
    Page(PageArgs),
}

/// Which database to use.
#[derive(Debug, Args)]
pub struct DatabaseArgs {
    /// The database file.
    #[arg(long, env = DATABASE_ENV, value_name = "PATH")]
    db: PathBuf,
}

/// Which source of a database to read, for databases that hold several.
#[derive(Debug, Args)]
pub struct SiteArgs {
    /// The site name of the source to read; only needed when the database
    /// holds more than one.
    #[arg(long, value_name = "SITE_NAME")]
    site: Option<String>,
}

/// Arguments of `wiki import`.
#[derive(Debug, Args)]
pub struct ImportArgs {
    /// The export file.
    #[arg(value_name = "EXPORT_XML")]
    export: PathBuf,
    #[command(flatten)]
    database: DatabaseArgs,
}

/// Arguments of `wiki search`.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// The words to search for; operators and punctuation are searched as text.
    #[arg(required = true, value_name = "WORD")]
    words: Vec<String>,
    /// The most hits to show.
    #[arg(long, default_value_t = 10, value_name = "N")]
    limit: usize,
    #[command(flatten)]
    database: DatabaseArgs,
    #[command(flatten)]
    site: SiteArgs,
}

/// Arguments of `wiki page`.
#[derive(Debug, Args)]
pub struct PageArgs {
    /// The page title; spaces and underscores are the same.
    #[arg(value_name = "TITLE")]
    title: String,
    #[command(flatten)]
    database: DatabaseArgs,
    #[command(flatten)]
    site: SiteArgs,
}

impl ImportArgs {
    /// The request these arguments describe.
    #[must_use]
    pub fn request(self) -> ImportRequest {
        ImportRequest {
            export: self.export,
            database: self.database.db,
        }
    }
}

impl SearchArgs {
    /// The request these arguments describe.
    ///
    /// # Errors
    ///
    /// Fails if the words or the limit are outside the search bounds.
    pub fn request(self) -> Result<SearchRequest, InvalidSearch> {
        let query = SearchQuery::new(&self.words.join(" "), self.limit)?;
        Ok(SearchRequest {
            corpus: corpus(self.database, self.site),
            query,
        })
    }
}

impl PageArgs {
    /// The request these arguments describe.
    #[must_use]
    pub fn request(self) -> PageRequest {
        PageRequest {
            corpus: corpus(self.database, self.site),
            title: self.title,
        }
    }
}

fn corpus(database: DatabaseArgs, site: SiteArgs) -> Corpus {
    Corpus {
        database: database.db,
        site: site.site,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use clap::error::ErrorKind;

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("osrs-mcp").chain(args.iter().copied()))
    }

    fn kind(args: &[&str]) -> ErrorKind {
        parse(args).unwrap_err().kind()
    }

    #[test]
    fn serve_takes_no_arguments() {
        assert!(matches!(parse(&["serve"]).unwrap().command, Command::Serve));
        assert_eq!(kind(&["serve", "extra"]), ErrorKind::UnknownArgument);
    }

    #[test]
    fn a_command_is_required_and_unknown_ones_are_refused() {
        assert_eq!(
            kind(&[]),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        assert_eq!(kind(&["--no-such-flag"]), ErrorKind::UnknownArgument);
        assert_eq!(
            kind(&["wiki"]),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        assert_eq!(kind(&["wiki", "frobnicate"]), ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn version_and_help_are_handled_by_the_parser() {
        assert_eq!(kind(&["--version"]), ErrorKind::DisplayVersion);
        assert_eq!(kind(&["-V"]), ErrorKind::DisplayVersion);
        assert_eq!(kind(&["--help"]), ErrorKind::DisplayHelp);
        assert_eq!(kind(&["wiki", "search", "--help"]), ErrorKind::DisplayHelp);
    }

    #[test]
    fn import_needs_a_file_and_a_database() {
        let Command::Wiki {
            command: WikiCommand::Import(args),
        } = parse(&["wiki", "import", "export.xml", "--db", "wiki.sqlite"])
            .unwrap()
            .command
        else {
            panic!("expected import");
        };
        let request = args.request();
        assert_eq!(request.export, PathBuf::from("export.xml"));
        assert_eq!(request.database, PathBuf::from("wiki.sqlite"));
        assert_eq!(
            kind(&["wiki", "import", "--db", "wiki.sqlite"]),
            ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn search_joins_its_words_and_applies_the_limit_and_site() {
        let Command::Wiki {
            command: WikiCommand::Search(args),
        } = parse(&[
            "wiki",
            "search",
            "fairy",
            "ring",
            "--limit",
            "3",
            "--db",
            "w.sqlite",
            "--site",
            "Example Wiki",
        ])
        .unwrap()
        .command
        else {
            panic!("expected search");
        };
        let request = args.request().unwrap();
        assert_eq!(request.query.limit(), 3);
        assert_eq!(request.corpus.site.as_deref(), Some("Example Wiki"));
        assert_eq!(request.corpus.database, PathBuf::from("w.sqlite"));
    }

    #[test]
    fn search_words_that_look_like_options_can_follow_a_double_dash() {
        let Command::Wiki {
            command: WikiCommand::Search(args),
        } = parse(&["wiki", "search", "--db", "w.sqlite", "--", "--limit", "x"])
            .unwrap()
            .command
        else {
            panic!("expected search");
        };
        assert_eq!(args.request().unwrap().query.limit(), 10);
    }

    #[test]
    fn search_bounds_are_checked_when_the_request_is_built() {
        let args = |extra: &[&str]| {
            let mut all = vec!["wiki", "search", "word", "--db", "w.sqlite"];
            all.extend_from_slice(extra);
            match parse(&all).unwrap().command {
                Command::Wiki {
                    command: WikiCommand::Search(args),
                } => args,
                other => panic!("expected search, got {other:?}"),
            }
        };
        assert_eq!(
            args(&["--limit", "0"]).request().unwrap_err(),
            InvalidSearch::ZeroLimit
        );
        assert_eq!(
            args(&["--limit", "51"]).request().unwrap_err(),
            InvalidSearch::LimitTooLarge
        );
        assert_eq!(
            kind(&["wiki", "search", "--db", "w.sqlite"]),
            ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn page_takes_one_title() {
        let Command::Wiki {
            command: WikiCommand::Page(args),
        } = parse(&["wiki", "page", "Dragon axe", "--db", "w.sqlite"])
            .unwrap()
            .command
        else {
            panic!("expected page");
        };
        assert_eq!(args.request().title, "Dragon axe");
        assert_eq!(
            kind(&["wiki", "page", "A", "B", "--db", "w.sqlite"]),
            ErrorKind::UnknownArgument
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_non_unicode_argument_is_a_usage_error_not_a_panic() {
        use std::os::unix::ffi::OsStringExt;

        let args = [
            OsString::from("osrs-mcp"),
            OsString::from_vec(b"--\xff".to_vec()),
        ];
        assert!(Cli::try_parse_from(args).is_err());
    }
}
