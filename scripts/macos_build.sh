#!/usr/bin/env bash
# GLM-F: reproducible clean-checkout release build for macOS.
# 1. frontend build (Bun: install + build → frontend assets for Tauri embedding)
# 2. cargo build --release --locked, raw exe copied to dist/ (AGENTS.md)
# 3. .app bundle via @tauri-apps/cli (valid package; tauri.conf.json is K4C's)
#    with ad-hoc signing, copied to dist/
# 4. embedded-frontend identity: the binary must embed exactly the current
#    frontend/dist asset set (vite content-hashed names) — a stale binary
#    with an older frontend is a hard FAIL (0.2.1 repair requirement).
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

echo "== 1/4 frontend build (Bun) =="
( cd frontend
  [[ -f package.json ]] || { echo "FAIL: frontend/package.json missing" >&2; exit 1; }
  bun install --frozen-lockfile
  bun run build
)
[[ -d frontend/dist ]] || { echo "FAIL: frontend build produced no dist assets" >&2; exit 1; }

# Embedded-identity manifest: what frontend/dist contained at embed time.
# vite content-hashes each bundle file, so the asset NAME already identifies
# the content; the manifest pins names+hashes for the acceptance report.
MANIFEST="dist/frontend-embed.sha256"
mkdir -p dist
( cd frontend/dist && find . -type f -print0 | sort -z | xargs -0 shasum -a 256 ) > "$MANIFEST"
echo "frontend manifest: $MANIFEST ($(grep -c . "$MANIFEST") files)"

echo "== 2/4 cargo build --release =="
cargo build --release --locked
EXE="target/release/aicouncil"
[[ -x "$EXE" ]] || { echo "FAIL: release exe missing after build" >&2; exit 1; }
cp "$EXE" "dist/aicouncil"
echo "== sha256 (dist/aicouncil) =="
shasum -a 256 "dist/aicouncil" | tee "dist/aicouncil.sha256"

# Embedded-frontend identity check (0.2.1): every dist file name must appear
# in the binary's embedded asset keys, and the embedded index-*.js set must
# equal dist's exactly (no leftover older vite bundle → no stale binary).
check_embed() {
    local bin="$1" label="$2" miss=0
    local rel
    # strings 一次性提取(不能 `strings | grep -q`:pipefail 下 SIGPIPE 会把
    # “匹配成功”误判为失败——0.2.0 已踩过这个坑,注释见 macos_release_check.sh)
    local embedded_names
    embedded_names="$(strings -a "$bin")"
    while IFS= read -r rel; do
        [[ -z "$rel" || "$rel" == "." ]] && continue
        if ! grep -qF "$rel" <<<"$embedded_names"; then
            echo "FAIL: $label does not embed frontend asset: $rel" >&2
            miss=1
        fi
    done < <(cut -d' ' -f3- "$MANIFEST" | sed 's|^[[:space:]]*||; s|^\./||')
    local embedded_js dist_js
    embedded_js="$(grep -oE 'assets/index-[A-Za-z0-9_-]+\.js' <<<"$embedded_names" | sort -u)"
    dist_js="$(cd frontend/dist && ls assets/index-*.js 2>/dev/null | sort -u)"
    if [[ "$embedded_js" != "$dist_js" ]]; then
        echo "FAIL: $label embedded JS set != frontend/dist (stale binary?)" >&2
        echo "  embedded: ${embedded_js:-<none>}" >&2
        echo "  dist:     ${dist_js:-<none>}" >&2
        miss=1
    fi
    if [[ $miss -ne 0 ]]; then
        echo "FAIL: embedded frontend identity check failed for $label" >&2
        exit 1
    fi
    echo "embedded frontend identity OK: $label ($(echo "$dist_js" | wc -l | tr -d ' ') JS bundle(s))"
}

if [[ "${1:-}" == "--no-bundle" ]]; then
    check_embed "dist/aicouncil" "raw exe"
    echo "OK: macos_build (exe only; bundle skipped by flag)"
    exit 0
fi

echo "== 3/4 .app bundle via @tauri-apps/cli (ad-hoc signed; NOT notarized) =="
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

echo "== 4/4 embedded frontend identity (raw exe + bundle binary) =="
check_embed "dist/aicouncil" "raw exe"
check_embed "dist/$APP_NAME/Contents/MacOS/aicouncil" "bundle binary"
echo "OK: macos_build"
