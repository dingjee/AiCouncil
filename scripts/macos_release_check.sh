#!/usr/bin/env bash
# GLM-F: verify the final dist artifacts (.app bundle and raw exe).
# STRICT: every expected artifact must exist and must claim version 0.2.0 —
# missing artifacts are a FAIL, never a SKIP. Checks: version, arm64 arch,
# codesign structure (ad-hoc expected; NOT notarization), obsolete GUI linkage.
# Usage: scripts/macos_release_check.sh [--app PATH] [--exe PATH]
#        [--skip-app] [--skip-exe] [--expected-version 0.2.0]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP="dist/AiCouncil.app"
EXE="dist/aicouncil"
EXPECTED_VERSION="0.2.0"
CHECK_APP=1
CHECK_EXE=1
while [[ $# -gt 0 ]]; do
    case "$1" in
        --app) APP="$2"; shift 2 ;;
        --exe) EXE="$2"; shift 2 ;;
        --skip-app) CHECK_APP=0; shift ;;
        --skip-exe) CHECK_EXE=0; shift ;;
        --expected-version) EXPECTED_VERSION="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

FAIL=0
die() { echo "FAIL: $1" >&2; FAIL=1; }
note() { echo "$1"; }

if [[ "$CHECK_APP" -eq 1 ]]; then
    if [[ ! -d "$APP" ]]; then
        die "expected .app bundle missing: $APP (build it with scripts/macos_build.sh)"
    else
        note "== .app bundle: $APP =="
        PLIST="$APP/Contents/Info.plist"
        BIN="$APP/Contents/MacOS/aicouncil"
        [[ -f "$PLIST" ]] || die "missing Info.plist"
        [[ -x "$BIN" ]] || die "missing executable Contents/MacOS/aicouncil"
        if [[ -f "$PLIST" ]]; then
            VER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST" 2>/dev/null || echo '')"
            note "version: ${VER:-<unreadable>}"
            [[ "$VER" == "$EXPECTED_VERSION" ]] || die "bundle version '$VER' != expected $EXPECTED_VERSION"
        fi
        if [[ -x "$BIN" ]]; then
            lipo -archs "$BIN" 2>/dev/null | grep -q arm64 || die "binary is not arm64"
            note "arch: arm64 ok"
            note "== codesign (structure only; ad-hoc expected, NOT notarization) =="
            codesign --verify --verbose=2 "$APP" 2>&1 | sed 's/^/  /' || die "codesign --verify failed"
            codesign -dv "$APP" 2>&1 | sed 's/^/  /' || true
            note "== linked libraries (must not contain obsolete iced/gtk/winit stacks) =="
            otool -L "$BIN" | sed 's/^/  /'
            if otool -L "$BIN" | grep -Ei "iced|gtk|gdk|wayland|x11" >/dev/null; then
                die "obsolete GUI linkage present"
            fi
        fi
        note "bundle size: $(du -sh "$APP" | awk '{print $1}')"
    fi
fi

if [[ "$CHECK_EXE" -eq 1 ]]; then
    if [[ ! -f "$EXE" ]]; then
        die "expected raw exe missing: $EXE (build it with scripts/macos_build.sh)"
    else
        note "== raw exe: $EXE =="
        shasum -a 256 "$EXE"
        lipo -archs "$EXE" 2>/dev/null | grep -q arm64 || die "exe is not arm64"
        note "arch: arm64 ok"
        # Raw exe carries no Info.plist; version probe matches the embedded
        # client string "AiCouncil/<version>" (heuristic; bundle plist is
        # authoritative). NB: no `grep -q` — it SIGPIPEs the pipe under
        # `set -o pipefail` and misreports matches.
        VERSION_LINE="$(strings -a "$EXE" | grep -F "AiCouncil/$EXPECTED_VERSION" | head -n1)"
        if [[ -n "$VERSION_LINE" ]]; then
            note "version string AiCouncil/$EXPECTED_VERSION present"
        else
            die "exe does not contain version string AiCouncil/$EXPECTED_VERSION"
        fi
        # Debug-only native smoke harness must be fully absent in release
        # (linker dead-strips the cfg(debug_assertions) module).
        SMOKE_HITS="$(strings -a "$EXE" | grep -c "AICOUNCIL_NATIVE_SMOKE" || true)"
        if [[ "$SMOKE_HITS" -ne 0 ]]; then
            die "release exe contains native smoke harness references ($SMOKE_HITS)"
        fi
        note "native smoke harness absent in release (AICOUNCIL_NATIVE_SMOKE refs: 0)"
    fi
fi

if [[ "$FAIL" -ne 0 ]]; then
    echo "RESULT: FAILED" >&2
    exit 1
fi
echo "RESULT: OK (version $EXPECTED_VERSION)"
