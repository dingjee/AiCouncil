#!/usr/bin/env bash
# GLM-F: controlled migration selfcheck against the REAL PC backup —
# without touching it. Verifies SHA256 of the backup (against its recorded
# manifest), copies db+config to a temp dir, then runs the baseline test
# (counts/integrity/idempotent import) against the COPY.
# Never reads or prints file contents; counts and hashes only.
# Usage: scripts/macos_migrate_selfcheck.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

BACKUP="$ROOT/dist/pc-data-backup"
[[ -f "$BACKUP/aicouncil.db" && -f "$BACKUP/SHA256.json" ]] || {
    echo "FAIL: $BACKUP is missing (aicouncil.db / SHA256.json)" >&2
    exit 1
}

echo "== verify backup hashes (manifest: SHA256.json) =="
for f in aicouncil.db config.json credentials.dpapi; do
    expected="$(bun -e "const j = require(process.argv[1]); const v = j['$f']; if (!v) process.exit(2); process.stdout.write(v);" "$BACKUP/SHA256.json")"
    actual="$(shasum -a 256 "$BACKUP/$f" | awk '{print $1}')"
    if [[ "$expected" != "$actual" ]]; then
        echo "FAIL: hash mismatch for $f (backup integrity)" >&2
        exit 1
    fi
    echo "ok: $f"
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/aicouncil-migrate-selfcheck.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
cp "$BACKUP/aicouncil.db" "$WORK/"
[[ -f "$BACKUP/aicouncil.db-wal" ]] && cp "$BACKUP/aicouncil.db-wal" "$WORK/"
[[ -f "$BACKUP/aicouncil.db-shm" ]] && cp "$BACKUP/aicouncil.db-shm" "$WORK/"

echo "== baseline migration selfcheck (on copy in $WORK) =="
export AICOUNCIL_BASELINE_DIR="$WORK"
cargo test --locked --test macos_baseline_backup -- --nocapture

echo "OK: macos_migrate_selfcheck (real backup untouched, copy deleted)"
