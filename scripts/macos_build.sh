#!/usr/bin/env bash
# GLM-F: reproducible clean-checkout release build for macOS.
# 1. frontend build (Bun: install + build → frontend assets for Tauri embedding)
# 2. cargo build --release --locked, raw exe copied to dist/ (AGENTS.md)
# 3. .app bundle via @tauri-apps/cli (valid package; tauri.conf.json is K4C's)
#    with ad-hoc signing, copied to dist/
#
# Serialization: run only when the lead says the root IPC/Tauri integration is
# ready (K4C coordinates the shared target/; one cargo build at a time).
# Usage: scripts/macos_build.sh [--no-bundle]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"

command -v bun >/dev/null || { echo "FAIL: bun not found (expected /opt/homebrew/bin/bun)" >&2; exit 1; }
command -v cargo >/dev/null || { echo "FAIL: cargo not found (expected $HOME/.cargo/bin/cargo)" >&2; exit 1; }
[[ -f tauri.conf.json ]] || { echo "FAIL: tauri.conf.json missing (K4C integration not landed)" >&2; exit 1; }

echo "== 1/3 frontend build (Bun) =="
( cd frontend
  [[ -f package.json ]] || { echo "FAIL: frontend/package.json missing" >&2; exit 1; }
  bun install --frozen-lockfile
  bun run build
)
[[ -d frontend/dist || -d dist ]] || { echo "FAIL: frontend build produced no dist assets" >&2; exit 1; }

echo "== 2/3 cargo build --release =="
mkdir -p dist
cargo build --release --locked
EXE="target/release/aicouncil"
[[ -x "$EXE" ]] || { echo "FAIL: release exe missing after build" >&2; exit 1; }
cp "$EXE" "dist/aicouncil"
echo "== sha256 (dist/aicouncil) =="
shasum -a 256 "dist/aicouncil" | tee "dist/aicouncil.sha256"

if [[ "${1:-}" == "--no-bundle" ]]; then
    echo "OK: macos_build (exe only; bundle skipped by flag)"
    exit 0
fi

echo "== 3/3 .app bundle via @tauri-apps/cli (ad-hoc signed; NOT notarized) =="
bunx --bun @tauri-apps/cli@^2 build --bundles app
BUNDLE_SRC="$(ls -d target/release/bundle/macos/*.app 2>/dev/null | head -n1 || true)"
[[ -n "$BUNDLE_SRC" ]] || { echo "FAIL: tauri produced no .app in target/release/bundle/macos" >&2; exit 1; }
# Seal resources properly: tauri's automatic ad-hoc signature can leave the
# bundle failing `codesign --verify` ("no resources but signature indicates
# they must be present"); re-sign ad-hoc explicitly (no notarization claimed).
codesign --force --deep -s - "$BUNDLE_SRC"
codesign --verify "$BUNDLE_SRC" || { echo "FAIL: codesign --verify after re-sign" >&2; exit 1; }
APP_NAME="$(basename "$BUNDLE_SRC")"
rm -rf "dist/$APP_NAME"
cp -R "$BUNDLE_SRC" "dist/$APP_NAME"
echo "bundle staged: dist/$APP_NAME"
echo "OK: macos_build"
