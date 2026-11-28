//! End-to-end check of the `wiki` commands through the built binary, using a
//! small synthetic export written to a temporary directory. It needs no
//! network, no real Wiki content and no local data.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const NAMESPACE: &str = "http://www.mediawiki.org/xml/export-0.11/";
const DATABASE_ENV: &str = "OSRS_MCP_WIKI_DB";

/// One synthetic page: id, title, revision, redirect target and text.
struct Page(u64, &'static str, u64, Option<&'static str>, &'static str);

fn export_xml(pages: &[Page]) -> String {
    let mut body = String::new();
    for Page(id, title, revision, redirect, text) in pages {
        let redirect = redirect.map_or_else(String::new, |target| {
            format!("<redirect title=\"{target}\" />")
        });
        let _ = write!(
            body,
            "<page><title>{title}</title><ns>0</ns><id>{id}</id>{redirect}<revision>\
             <id>{revision}</id><timestamp>2026-03-04T05:06:07Z</timestamp>\
             <model>wikitext</model><format>text/x-wiki</format>\
             <text bytes=\"{}\" xml:space=\"preserve\">{text}</text></revision></page>",
            text.len()
        );
    }
    format!(
        "<mediawiki xmlns=\"{NAMESPACE}\" version=\"0.11\" xml:lang=\"en\">\
         <siteinfo><sitename>Example Wiki</sitename><dbname>examplewiki</dbname>\
         <base>https://wiki.example/w/Main_Page</base><generator>MediaWiki 1.45.3</generator>\
         <case>first-letter</case><namespaces><namespace key=\"0\" case=\"first-letter\" />\
         </namespaces></siteinfo>{body}</mediawiki>"
    )
}

struct Workspace {
    directory: TempDir,
}

impl Workspace {
    fn new() -> Self {
        Self {
            directory: TempDir::new().expect("temporary directory"),
        }
    }

    fn database(&self) -> PathBuf {
        self.directory.path().join("wiki.sqlite")
    }

    fn export(&self, name: &str, pages: &[Page]) -> PathBuf {
        let path = self.directory.path().join(name);
        fs::write(&path, export_xml(pages)).expect("write export");
        path
    }

    fn run(args: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_osrs-mcp"))
            .args(args)
            .env_remove(DATABASE_ENV)
            .output()
            .expect("binary should start");
        Run::new(output)
    }

    fn run_with_db(&self, args: &[&str]) -> Run {
        let database = self.database();
        let mut all = args.to_vec();
        all.extend(["--db", database.to_str().expect("UTF-8 path")]);
        Self::run(&all)
    }

    fn import(&self, path: &Path) -> Run {
        self.run_with_db(&["wiki", "import", path.to_str().expect("UTF-8 path")])
    }
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn new(output: Output) -> Self {
        Self {
            code: output.status.code(),
            stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        }
    }
}

fn first_export(workspace: &Workspace) -> PathBuf {
    workspace.export(
        "first.xml",
        &[
            Page(1, "Axe", 10, None, "== Uses ==\nChops trees and logs."),
            Page(2, "Hatchet", 20, Some("Axe"), "#REDIRECT [[Axe]]"),
            Page(
                3,
                "Dangling",
                30,
                Some("Gone page"),
                "#REDIRECT [[Gone page]]",
            ),
        ],
    )
}

#[test]
fn import_search_and_page_work_end_to_end_on_a_synthetic_export() {
    let workspace = Workspace::new();
    let export = first_export(&workspace);

    let imported = workspace.import(&export);
    assert_eq!(imported.code, Some(0), "stderr: {}", imported.stderr);
    assert!(imported.stdout.contains("Imported pages from Example Wiki"));
    assert!(imported.stdout.contains("3 pages, 2 of them redirects"));
    assert!(
        imported
            .stdout
            .contains("Stored: 3 added, 0 replaced, 0 unchanged")
    );
    assert!(workspace.database().is_file());

    let again = workspace.import(&export);
    assert_eq!(again.code, Some(0));
    assert!(
        again
            .stdout
            .contains("Stored: 0 added, 0 replaced, 3 unchanged")
    );

    let found = workspace.run_with_db(&["wiki", "search", "trees", "logs"]);
    assert_eq!(found.code, Some(0), "stderr: {}", found.stderr);
    assert!(found.stdout.contains("1. Axe > Uses"), "{}", found.stdout);
    assert!(
        found.stdout.contains("page 1, revision 10"),
        "{}",
        found.stdout
    );
    assert!(found.stdout.contains("Chops trees and logs."));

    let none = workspace.run_with_db(&["wiki", "search", "trees", "absentword"]);
    assert_eq!(none.code, Some(0));
    assert_eq!(none.stdout, "No matches.\n");

    let direct = workspace.run_with_db(&["wiki", "page", "axe"]);
    assert_eq!(direct.code, Some(0), "stderr: {}", direct.stderr);
    assert!(
        direct.stdout.starts_with("Axe\npage 1, revision 10"),
        "{}",
        direct.stdout
    );

    let redirected = workspace.run_with_db(&["wiki", "page", "Hatchet"]);
    assert_eq!(redirected.code, Some(0));
    assert!(
        redirected
            .stdout
            .starts_with("Axe (redirected from Hatchet)\n"),
        "{}",
        redirected.stdout
    );
    assert!(redirected.stdout.contains("Chops trees and logs."));
}

#[test]
fn a_title_that_does_not_lead_to_a_page_fails_with_a_specific_message() {
    let workspace = Workspace::new();
    workspace.import(&first_export(&workspace));

    let unknown = workspace.run_with_db(&["wiki", "page", "Nothing here"]);
    assert_eq!(unknown.code, Some(1));
    assert_eq!(unknown.stdout, "");
    assert!(
        unknown.stderr.contains("No page has that title."),
        "{}",
        unknown.stderr
    );

    let dangling = workspace.run_with_db(&["wiki", "page", "Dangling"]);
    assert_eq!(dangling.code, Some(1));
    assert_eq!(dangling.stdout, "");
    assert!(
        dangling
            .stderr
            .contains("\"Gone page\", which is not in the database"),
        "{}",
        dangling.stderr
    );
}

#[test]
fn a_stale_revision_fails_the_whole_import_and_leaves_the_database_as_it_was() {
    let workspace = Workspace::new();
    workspace.import(&first_export(&workspace));
    let stale = workspace.export(
        "stale.xml",
        &[
            Page(4, "Pick", 40, None, "Mines rocks."),
            Page(1, "Axe", 5, None, "Older text."),
        ],
    );

    let failed = workspace.import(&stale);
    assert_eq!(failed.code, Some(1));
    assert_eq!(failed.stdout, "");
    assert!(
        failed.stderr.contains("newer than offered revision"),
        "{}",
        failed.stderr
    );
    assert!(
        !failed.stderr.contains("Axe"),
        "no title in errors: {}",
        failed.stderr
    );

    let rocks = workspace.run_with_db(&["wiki", "search", "rocks"]);
    assert_eq!(
        rocks.stdout, "No matches.\n",
        "the first page of the failed file was kept"
    );
    let page = workspace.run_with_db(&["wiki", "page", "Axe"]);
    assert!(page.stdout.contains("Chops trees and logs."));
}

#[test]
fn an_updated_export_replaces_the_page_and_search_follows() {
    let workspace = Workspace::new();
    workspace.import(&first_export(&workspace));
    let newer = workspace.export(
        "newer.xml",
        &[Page(1, "Axe", 11, None, "Now it fells timber.")],
    );

    let updated = workspace.import(&newer);
    assert_eq!(updated.code, Some(0), "stderr: {}", updated.stderr);
    assert!(
        updated
            .stdout
            .contains("Stored: 0 added, 1 replaced, 0 unchanged")
    );

    let old = workspace.run_with_db(&["wiki", "search", "trees"]);
    assert_eq!(old.stdout, "No matches.\n");
    let new = workspace.run_with_db(&["wiki", "search", "timber"]);
    assert!(new.stdout.contains("revision 11"), "{}", new.stdout);
}

#[test]
fn reading_commands_never_create_a_missing_database() {
    let workspace = Workspace::new();
    for args in [["wiki", "search", "trees"], ["wiki", "page", "Axe"]] {
        let run = workspace.run_with_db(&args);
        assert_eq!(run.code, Some(1));
        assert!(
            run.stderr.contains("database file does not exist"),
            "{}",
            run.stderr
        );
    }
    assert!(!workspace.database().exists());
}

#[test]
fn a_bad_export_file_fails_cleanly_without_creating_a_database() {
    let workspace = Workspace::new();
    let missing = workspace.directory.path().join("absent.xml");
    let run = workspace.import(&missing);
    assert_eq!(run.code, Some(1));
    assert!(
        run.stderr.contains("export file could not be opened"),
        "{}",
        run.stderr
    );

    let garbage = workspace.directory.path().join("garbage.xml");
    fs::write(&garbage, "not xml at all").expect("write file");
    let run = workspace.import(&garbage);
    assert_eq!(run.code, Some(1));
    assert!(run.stderr.contains("could not be read"), "{}", run.stderr);
    assert!(!workspace.database().exists());
}

#[test]
fn the_database_can_come_from_the_environment_instead_of_a_flag() {
    let workspace = Workspace::new();
    let export = first_export(&workspace);
    let output = Command::new(env!("CARGO_BIN_EXE_osrs-mcp"))
        .args(["wiki", "import", export.to_str().expect("UTF-8 path")])
        .env(DATABASE_ENV, workspace.database())
        .output()
        .expect("binary should start");
    assert_eq!(output.status.code(), Some(0));
    assert!(workspace.database().is_file());
}

#[test]
fn missing_or_invalid_arguments_are_usage_errors_on_stderr_only() {
    for args in [
        &["wiki", "search", "--db", "x.sqlite"][..],
        &["wiki", "search", "word", "--db", "x.sqlite", "--limit", "0"][..],
        &["wiki", "page", "Axe"][..],
        &["wiki", "frobnicate"][..],
    ] {
        let run = Workspace::run(args);
        assert_eq!(run.code, Some(2), "{args:?}: {}", run.stderr);
        assert_eq!(run.stdout, "", "{args:?}");
        assert!(!run.stderr.is_empty(), "{args:?}");
    }
}
