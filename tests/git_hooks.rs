//! Behavioural tests for the opt-in Git hooks and their installer.
//!
//! Every test builds a throwaway repository in a unique temporary directory,
//! copies in the real hooks and installer, and replaces `scripts/check.sh` with
//! a stub that records its arguments. The tests therefore never touch the
//! developer's own repository or Git configuration, and never recurse into the
//! real checks.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT_ID: AtomicU32 = AtomicU32::new(0);

/// A temporary repository (plus bare remote) that is deleted on drop.
struct Fixture {
    root: PathBuf,
    work: PathBuf,
    remote: PathBuf,
}

impl Fixture {
    /// Creates a repository whose stubbed `scripts/check.sh` exits with `check_status`.
    fn new(check_status: u8) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("osrs-mcp-hooks-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let work = root.join("work");
        let remote = root.join("remote.git");
        fs::create_dir_all(work.join("scripts")).unwrap();
        fs::create_dir_all(&remote).unwrap();

        let fixture = Self { root, work, remote };
        fixture.git_ok(&fixture.remote, &["init", "--bare", "--quiet"]);
        fixture.git_ok(&fixture.work, &["init", "--quiet", "--initial-branch=main"]);
        fixture.git_ok(&fixture.work, &["config", "user.name", "Test"]);
        fixture.git_ok(&fixture.work, &["config", "user.email", "test@example.com"]);
        fixture.git_ok(&fixture.work, &["config", "commit.gpgsign", "false"]);
        fixture.git_ok(
            &fixture.work,
            &["remote", "add", "origin", fixture.remote.to_str().unwrap()],
        );

        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        fixture.copy_from(manifest, ".githooks/pre-commit");
        fixture.copy_from(manifest, ".githooks/pre-push");
        fixture.copy_from(manifest, "scripts/install-hooks.sh");
        fixture.write_check_stub(check_status);
        fixture.write("README.md", "# Fixture\n");
        fixture.git_ok(&fixture.work, &["add", "."]);
        fixture.git_ok(&fixture.work, &["commit", "--quiet", "-m", "initial"]);
        fixture
    }

    fn copy_from(&self, manifest: &Path, relative: &str) {
        let target = self.work.join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(manifest.join(relative), target).unwrap();
    }

    fn write_check_stub(&self, status: u8) {
        let stub = format!(
            "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/../check.log\"\nexit {status}\n"
        );
        self.write("scripts/check.sh", &stub);
        make_executable(&self.work.join("scripts/check.sh"));
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.work.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// Arguments of each recorded `scripts/check.sh` invocation, one entry per call.
    fn check_calls(&self) -> Vec<String> {
        fs::read_to_string(self.work.join("check.log")).map_or_else(
            |_| Vec::new(),
            |log| log.lines().map(str::to_owned).collect(),
        )
    }

    fn assert_no_checks_ran(&self) {
        assert_eq!(self.check_calls(), Vec::<String>::new());
    }

    /// A Git command isolated from the host's configuration and from any
    /// `GIT_*` variables inherited when these tests run inside a Git hook.
    fn git(&self, dir: &Path, args: &[&str]) -> Output {
        let mut command = Command::new("git");
        command.current_dir(dir).args(args);
        for var in local_git_env_vars() {
            command.env_remove(var);
        }
        command
            .env("GIT_CONFIG_GLOBAL", self.global_config())
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("HOME", &self.root)
            .output()
            .expect("git should start")
    }

    fn git_ok(&self, dir: &Path, args: &[&str]) -> String {
        let output = self.git(dir, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn global_config(&self) -> PathBuf {
        self.root.join("global.gitconfig")
    }

    fn run_installer(&self, args: &[&str]) -> Output {
        let mut command = Command::new("sh");
        command
            .current_dir(&self.work)
            .arg("scripts/install-hooks.sh")
            .args(args);
        for var in local_git_env_vars() {
            command.env_remove(var);
        }
        command
            .env("GIT_CONFIG_GLOBAL", self.global_config())
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("HOME", &self.root)
            .output()
            .expect("installer should start")
    }

    /// The repository-local `core.hooksPath`, if set.
    fn local_hooks_path(&self) -> Option<String> {
        let output = self.git(
            &self.work,
            &["config", "--local", "--get", "core.hooksPath"],
        );
        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
    }

    fn install_hooks(&self) {
        let output = self.run_installer(&[]);
        assert!(
            output.status.success(),
            "install failed: {}",
            stderr(&output)
        );
    }

    fn stage(&self, relative: &str, contents: &str) {
        self.write(relative, contents);
        self.git_ok(&self.work, &["add", relative]);
    }

    fn commit(&self, message: &str) -> Output {
        self.git(&self.work, &["commit", "--quiet", "-m", message])
    }

    fn push(&self) -> Output {
        self.git(&self.work, &["push", "--quiet", "origin", "main"])
    }

    fn remote_has_main(&self) -> bool {
        self.git(
            &self.remote,
            &["rev-parse", "--verify", "--quiet", "refs/heads/main"],
        )
        .status
        .success()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Variables Git sets for hooks that would redirect a nested `git` to the wrong repository.
fn local_git_env_vars() -> Vec<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--local-env-vars"])
        .output()
        .expect("git should start");
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn shipped_hooks_are_executable() {
    use std::os::unix::fs::PermissionsExt;

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for relative in [
        ".githooks/pre-commit",
        ".githooks/pre-push",
        "scripts/install-hooks.sh",
    ] {
        let mode = fs::metadata(manifest.join(relative))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(
            mode & 0o111,
            0,
            "{relative} must be executable (mode {mode:o})"
        );
    }
}

// --- installer ---------------------------------------------------------

#[test]
fn install_sets_repository_local_hooks_path() {
    let repo = Fixture::new(0);

    let output = repo.run_installer(&[]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(repo.local_hooks_path().as_deref(), Some(".githooks"));
}

#[test]
fn install_is_idempotent() {
    let repo = Fixture::new(0);
    repo.install_hooks();

    let output = repo.run_installer(&[]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(repo.local_hooks_path().as_deref(), Some(".githooks"));
    assert!(
        stdout(&output).contains("already installed"),
        "stdout: {}",
        stdout(&output)
    );
}

#[test]
fn install_never_writes_global_git_configuration() {
    let repo = Fixture::new(0);
    fs::write(repo.global_config(), "[user]\n\tname = Someone\n").unwrap();

    repo.install_hooks();

    let global = fs::read_to_string(repo.global_config()).unwrap();
    assert_eq!(global, "[user]\n\tname = Someone\n");
}

#[test]
fn install_refuses_to_overwrite_a_custom_hooks_path() {
    let repo = Fixture::new(0);
    repo.git_ok(&repo.work, &["config", "core.hooksPath", "my-hooks"]);

    let output = repo.run_installer(&[]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("my-hooks"),
        "stderr: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("--replace"),
        "stderr: {}",
        stderr(&output)
    );
    assert_eq!(repo.local_hooks_path().as_deref(), Some("my-hooks"));
}

#[test]
fn install_refuses_when_a_global_hooks_path_would_be_shadowed() {
    let repo = Fixture::new(0);
    fs::write(
        repo.global_config(),
        "[core]\n\thooksPath = /opt/shared-hooks\n",
    )
    .unwrap();

    let output = repo.run_installer(&[]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("/opt/shared-hooks"),
        "stderr: {}",
        stderr(&output)
    );
    assert_eq!(repo.local_hooks_path(), None);
}

#[test]
fn install_replaces_a_custom_hooks_path_only_with_explicit_consent() {
    let repo = Fixture::new(0);
    repo.git_ok(&repo.work, &["config", "core.hooksPath", "my-hooks"]);

    let output = repo.run_installer(&["--replace"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(repo.local_hooks_path().as_deref(), Some(".githooks"));
}

#[test]
fn install_warns_about_existing_hooks_it_would_bypass() {
    let repo = Fixture::new(0);
    let legacy = repo.work.join(".git/hooks/commit-msg");
    fs::write(&legacy, "#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&legacy);

    let output = repo.run_installer(&[]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("commit-msg"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn uninstall_removes_only_this_repositorys_hooks_path() {
    let repo = Fixture::new(0);
    repo.install_hooks();

    let output = repo.run_installer(&["uninstall"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(repo.local_hooks_path(), None);
}

#[test]
fn uninstall_leaves_a_custom_hooks_path_alone() {
    let repo = Fixture::new(0);
    repo.git_ok(&repo.work, &["config", "core.hooksPath", "my-hooks"]);

    let output = repo.run_installer(&["uninstall"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(repo.local_hooks_path().as_deref(), Some("my-hooks"));
}

#[test]
fn uninstall_when_nothing_is_installed_succeeds() {
    let repo = Fixture::new(0);

    let output = repo.run_installer(&["uninstall"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
}

#[test]
fn unknown_installer_argument_fails_with_usage() {
    let repo = Fixture::new(0);

    let output = repo.run_installer(&["--bogus"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("Usage:"),
        "stderr: {}",
        stderr(&output)
    );
}

// --- pre-commit --------------------------------------------------------

#[test]
fn commit_is_unaffected_until_hooks_are_installed() {
    let repo = Fixture::new(1);
    repo.stage("notes.md", "trailing space \n");

    let output = repo.commit("opt-in only");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    repo.assert_no_checks_ran();
}

#[test]
fn pre_commit_rejects_staged_whitespace_errors() {
    let repo = Fixture::new(0);
    repo.install_hooks();
    repo.stage("notes.md", "trailing space \n");

    let output = repo.commit("bad whitespace");

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("trailing whitespace"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn pre_commit_ignores_whitespace_that_is_not_staged() {
    let repo = Fixture::new(0);
    repo.install_hooks();
    repo.stage("notes.md", "clean\n");
    repo.write("notes.md", "clean\nunstaged trailing \n");

    let output = repo.commit("staged content is clean");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
}

#[test]
fn pre_commit_checks_formatting_when_rust_files_are_staged() {
    let repo = Fixture::new(0);
    repo.install_hooks();
    repo.stage("src/lib.rs", "pub fn f() {}\n");

    let output = repo.commit("add rust");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(repo.check_calls(), ["fmt"]);
}

#[test]
fn pre_commit_blocks_the_commit_when_formatting_fails() {
    let repo = Fixture::new(1);
    repo.install_hooks();
    repo.stage("src/lib.rs", "pub fn   f(){}\n");

    let output = repo.commit("unformatted");

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cargo fmt"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn pre_commit_skips_formatting_when_no_rust_files_are_staged() {
    let repo = Fixture::new(1);
    repo.install_hooks();
    repo.stage("notes.md", "docs only\n");

    let output = repo.commit("docs");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    repo.assert_no_checks_ran();
}

#[test]
fn pre_commit_never_modifies_files() {
    let repo = Fixture::new(1);
    repo.install_hooks();
    repo.stage("src/lib.rs", "pub fn   f(){}   \n");

    let _ = repo.commit("rejected");

    let contents = fs::read_to_string(repo.work.join("src/lib.rs")).unwrap();
    assert_eq!(contents, "pub fn   f(){}   \n");
    assert_eq!(repo.git_ok(&repo.work, &["diff", "--name-only"]), "");
}

#[test]
fn no_verify_bypasses_pre_commit() {
    let repo = Fixture::new(1);
    repo.install_hooks();
    repo.stage("notes.md", "trailing space \n");

    let output = repo.git(
        &repo.work,
        &["commit", "--quiet", "--no-verify", "-m", "skip"],
    );

    assert!(output.status.success(), "stderr: {}", stderr(&output));
}

// --- pre-push ----------------------------------------------------------

#[test]
fn pre_push_runs_the_full_check_script_and_allows_the_push() {
    let repo = Fixture::new(0);
    repo.install_hooks();

    let output = repo.push();

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(
        repo.check_calls(),
        [""],
        "full check.sh run without arguments"
    );
    assert!(repo.remote_has_main());
}

#[test]
fn pre_push_blocks_the_push_when_checks_fail() {
    let repo = Fixture::new(1);
    repo.install_hooks();

    let output = repo.push();

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("pre-push"),
        "stderr: {}",
        stderr(&output)
    );
    assert!(!repo.remote_has_main());
}

#[test]
fn pre_push_warns_when_the_working_tree_differs_from_what_is_pushed() {
    let repo = Fixture::new(0);
    repo.install_hooks();
    repo.write("README.md", "# Fixture\nuncommitted change\n");

    let output = repo.push();

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("uncommitted"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn pre_push_is_quiet_when_the_working_tree_is_clean() {
    let repo = Fixture::new(0);
    repo.install_hooks();

    let output = repo.push();

    assert!(
        !stderr(&output).contains("uncommitted"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn pre_push_skips_checks_when_only_deleting_remote_refs() {
    let repo = Fixture::new(1);
    repo.git_ok(
        &repo.work,
        &["push", "--quiet", "--no-verify", "origin", "main:old"],
    );
    repo.install_hooks();

    let output = repo.git(
        &repo.work,
        &["push", "--quiet", "origin", "--delete", "old"],
    );

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    repo.assert_no_checks_ran();
}
