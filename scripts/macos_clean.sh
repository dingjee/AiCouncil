#!/usr/bin/env bash
# GLM-F: cleanup of TASK-CREATED temp/cache/test artifacts only.
# Inventory of record: docs/MACOS_ACCEPTANCE.md. By default this prints what it
# WOULD remove (dry run); pass --apply to delete, --target to also remove
# target/ (final cleanup step; never during active parallel work).
#
# NEVER touched by this script: dist/data, dist/pc-data-backup, .env,
# ~/.cargo, ~/.rustup, shared Bun caches, node_modules (MM-F owns), OpenCode
# sessions, pre-existing user caches.
# Usage: scripts/macos_clean.sh [--apply] [--target]
set -euo pipefail

APPLY=0
TARGET=0
for arg in "$@"; do
    case "$arg" in
        --apply) APPLY=1 ;;
        --target) TARGET=1 ;;
        *) echo "unknown flag: $arg" >&2; exit 2 ;;
    esac
done

remove() {
    local path="$1"
    if [[ ! -e "$path" ]]; then
        return 0
    fi
    if [[ "$APPLY" -eq 1 ]]; then
        rm -rf "$path"
        echo "removed: $path"
    else
        echo "would remove: $path"
    fi
}

echo "== task-created temp artifacts (dry run: $((1 - APPLY)); use --apply to delete) =="
# Isolated test/check data dirs created by macos_check.sh / selfcheck (belt &
# braces; their traps normally already removed them)
for pattern in 'aicouncil-check.*' 'aicouncil-migrate-selfcheck.*'; do
    while IFS= read -r d; do
        remove "$d"
    done < <(find "${TMPDIR:-/tmp}" -maxdepth 1 -name "$pattern" 2>/dev/null || true)
done

if [[ "$TARGET" -eq 1 ]]; then
    echo "== target/ (final cleanup only — shared with K4C; confirm parallel builds done) =="
    remove "$(dirname "${BASH_SOURCE[0]}")/../target"
else
    echo "SKIP: target/ kept (pass --target at final cleanup; coordinate with K4C)"
fi

echo "Preserved by policy: dist/data, dist/pc-data-backup, .env, ~/.cargo, ~/.rustup,"
echo "shared caches, frontend/node_modules, OpenCode sessions."
echo "Done (apply=$APPLY)."
