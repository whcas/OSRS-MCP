#!/bin/sh
# Opt-in installer for the repository's native Git hooks (see `.githooks/`).
#
# Usage: scripts/install-hooks.sh [install|uninstall] [--replace]
#
# Sets `core.hooksPath` in this repository's local config only; global and
# system Git configuration are never written. Installing is idempotent. An
# existing different `core.hooksPath` (from any config scope) is never
# overwritten unless `--replace` is given.

hooks_dir=.githooks

usage() {
    echo "Usage: $0 [install|uninstall] [--replace]" >&2
}

action=install
replace=false
for arg in "$@"; do
    case "$arg" in
        install | uninstall) action=$arg ;;
        --replace) replace=true ;;
        *)
            usage
            exit 2
            ;;
    esac
done

root=$(git rev-parse --show-toplevel) || exit 1
cd "$root" || exit 1

local_path=$(git config --local --get core.hooksPath)
effective_path=$(git config --get core.hooksPath)

install_hooks() {
    if [ "$effective_path" = "$hooks_dir" ]; then
        echo "Git hooks already installed (core.hooksPath=$hooks_dir)."
        return 0
    fi
    if [ -n "$effective_path" ] && ! $replace; then
        echo "core.hooksPath is already set to '$effective_path'; not overwriting it." >&2
        echo "Re-run with --replace to use $hooks_dir in this repository instead." >&2
        return 1
    fi
    warn_about_bypassed_hooks
    git config --local core.hooksPath "$hooks_dir" || return 1
    echo "Installed: core.hooksPath=$hooks_dir (this repository only)."
    echo "Remove with: $0 uninstall"
}

# Hooks in the default directory stop running once hooksPath points elsewhere.
warn_about_bypassed_hooks() {
    default_dir=$(git rev-parse --git-path hooks)
    for hook in "$default_dir"/*; do
        [ -f "$hook" ] || continue
        case "$hook" in *.sample) continue ;; esac
        echo "warning: $hook will no longer run while $hooks_dir is active." >&2
    done
}

uninstall_hooks() {
    if [ "$local_path" != "$hooks_dir" ]; then
        echo "Nothing to remove: this repository's core.hooksPath is not $hooks_dir."
        return 0
    fi
    git config --local --unset core.hooksPath || return 1
    echo "Removed core.hooksPath=$hooks_dir."
}

case "$action" in
    install) install_hooks ;;
    uninstall) uninstall_hooks ;;
esac
