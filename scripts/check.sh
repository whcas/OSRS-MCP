#!/usr/bin/env bash
# Shared check command for local development and CI.
#
# Usage: scripts/check.sh [fmt|clippy|test|audit|all]
#
# `all` (the default) runs fmt, clippy and test. `audit` is run separately: it
# needs network access to fetch the RustSec advisory database, and cargo-deny
# at the version CI uses:
#
#   cargo install --locked --version 0.20.2 cargo-deny
set -euo pipefail

cd "$(dirname "$0")/.."

run() {
    printf '\n==> %s\n' "$*"
    "$@"
}

check_fmt() {
    run cargo fmt --all --check
}

check_clippy() {
    run cargo clippy --locked --all-targets --all-features -- -D warnings
}

check_test() {
    run cargo build --locked --all-targets
    run cargo test --locked
}

check_audit() {
    if ! command -v cargo-deny >/dev/null 2>&1; then
        echo "cargo-deny is not installed; run: cargo install --locked --version 0.20.2 cargo-deny" >&2
        exit 1
    fi
    run cargo deny --locked check advisories bans sources
}

usage() {
    echo "Usage: $0 [fmt|clippy|test|audit|all]" >&2
}

case "${1:-all}" in
    fmt) check_fmt ;;
    clippy) check_clippy ;;
    test) check_test ;;
    audit) check_audit ;;
    all)
        check_fmt
        check_clippy
        check_test
        ;;
    -h | --help)
        usage
        ;;
    *)
        usage
        exit 2
        ;;
esac
