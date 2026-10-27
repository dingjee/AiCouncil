#!/usr/bin/env bash
# GLM-F: macOS check gate — fmt, clippy, tests. Test data is fully isolated:
# AICOUNCIL_DATA_DIR points at a temp dir for the whole run, so no test can
# touch the real user data in dist/data or Application Support.
# Usage: scripts/macos_check.sh [--quick]   (--quick skips clippy on all targets)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

ISOLATION="$(mktemp -d "${TMPDIR:-/tmp}/aicouncil-check.XXXXXX")"
export AICOUNCIL_DATA_DIR="$ISOLATION"
trap 'rm -rf "$ISOLATION"' EXIT

echo "== cargo fmt --all -- --check =="
cargo fmt --all -- --check

if [[ "${1:-}" != "--quick" ]]; then
    echo "== cargo clippy --all-targets -- -D warnings =="
    cargo clippy --all-targets --locked -- -D warnings
fi

echo "== cargo test (isolated data dir: $ISOLATION) =="
cargo test --locked
echo "OK: macos_check"
