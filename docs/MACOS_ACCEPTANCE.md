# macOS acceptance report (GLM-F scope)

Owner: GLM-F (final release checks). Date: 2026-10-28. Branch: `codex/tauri-macos`.
Version: 0.2.0 (Tauri 2, Apple Silicon, `custom-protocol` embedded frontend).
This report lists **actual commands and results**. Not-run items are stated as such.
Current-state document only; superseded Windows-era claims removed.

## Environment

- Rust: rustup `stable-aarch64-apple-darwin`, **1.99.1**, `/Users/jee/.cargo/bin`
  (installed this task; clippy + rustfmt components added). `rust-toolchain.toml`
  pins `channel = "stable"`.
- Bun 1.4.4 (`/opt/homebrew/bin/bun`, pre-existing). node/npm absent — Bun only.
- Real user data and real credential values were never read by GLM-F; all
  tests use temp dirs and `sk-fixture-*` fixtures.

## Data location & migration (implemented, `src/storage/repo.rs`)

- `AICOUNCIL_DATA_DIR` set → exactly that directory; no legacy import.
- Bundled app → `~/Library/Application Support/AiCouncil`; first launch imports
  sibling `../data/{aicouncil.db,config.json}` via staged-copy → WAL recovery on
  the copy → SQLite backup snapshot → `quick_check` + `foreign_key_check` →
  atomic rename. Non-overwrite, idempotent, source bytes unchanged; corrupt
  source → error, no target created. `credentials.dpapi` is never imported or
  decrypted (inert; original preserved in `dist/pc-data-backup/`).

## Credentials (implemented, `src/storage/credentials.rs`)

- `.env` in the stable data dir is the sole source; allowlisted variables
  `OPENROUTER_API_KEY`, `DEEPSEEK_API_KEY`. Selection identical to desktop
  `env_keys` (`AICOUNCIL_ENV_PATH` > data dir > exe sibling; strict isolation
  under `AICOUNCIL_DATA_DIR`; first existing file is authoritative; empty/missing
  key = not configured, no fall-through). Reload = re-read per lookup; the
  in-app reload rebuilds provider/title clients. No Keychain/keyring/DPAPI;
  no write/delete API (removed with the abandoned 0.1.x surface).

## Verified results (actual)

| Check | Command | Result |
| --- | --- | --- |
| rustfmt | `cargo fmt --all -- --check` | PASS |
| Clippy strict | `cargo clippy --all-targets --locked -- -D warnings` | PASS |
| Full Rust suite | `cargo test --locked` (isolated `AICOUNCIL_DATA_DIR`) | PASS — 12 suites, 171 tests, 0 failed (incl. storage/repo unit, migration chain, composer, desktop) |
| Backup integrity + migration | `scripts/macos_migrate_selfcheck.sh` | PASS — manifest SHA256 match (db/config/dpapi); copy imports with identical counts 3 conv / 18 msg / 12 gen / 12 usage / 2 groups / 6 migrations; `integrity_check ok`; idempotent; original untouched |
| Wire contract | `docs/TAURI_IPC.md` v1 | Implemented backend matches (lead-owned); smoke exercised bootstrap/settings/groups/conversations/rename/draft CAS/catalog/preferences/key status/send/stream/cancel/export/reload with real invocations |
| Frontend typecheck/build | `tsc --noEmit && vite build` | PASS at release-build time (MM-F-owned suite also green per lead/frontend) |

## Native smoke (real WKWebView + real IPC) — status: **GATE OPEN, owned by MM-F**

Best run (pre menu/save-dialog refactor): 18 steps PASSED with real IPC —
bootstrap (fixture credentials visible, version 0.2.0), group/conversation CRUD,
rename CAS, byte-exact draft roundtrip + revision semantics, settings merge,
error-code convention (`not_found`), mock catalog + preference, key status,
`send_message` → SSE stream persisted (`completed`, cost `0.000014`, content
match), slow-send `cancel_request` (`cancelled`), `export_markdown` content,
`reload_credentials`, DOM rendered the conversation.
Synthetic IME gate verified up to `composing_enter_blocked_confirmed`
(Enter with `isComposing:true` does NOT send); its normal-Enter leg is the
open item. **Current blocker (MM-F, diagnosis in docs/GLMF_STATUS.md):**
harness body no longer executes after the menu/save-dialog changes — zero
mock requests, zero `smoke_report` reaches Rust, 8× fallback injections.
Display-sleep throttling ruled out (reproduced under caffeinate assertions).
Full IPC gate must pass before final dist staging; IME note: the harness test
is SYNTHETIC DOM events; a manual physical Pinyin-candidate test was NOT
performed and is not claimed.

## Release artifacts (built; dist staging DEFERRED until native gate)

Actual build (02:58–03:05): frontend `bun install --frozen-lockfile && bun run
build` (index-C5h9wlqI.js, 702.71 kB) → `cargo build --release --locked`
(finished 1m41s) → `bunx --bun @tauri-apps/cli@^2 build --bundles app` →
`target/release/bundle/macos/AiCouncil.app` (20.52 MiB, ad-hoc signed).
Note: tauri's automatic signature failed `codesign --verify` ("code has no
resources but signature indicates they must be present"); the build script now
re-signs explicitly (`codesign --force --deep -s -`) and verifies — fixed for
reproducibility.

Strict check `scripts/macos_release_check.sh --app
target/release/bundle/macos/AiCouncil.app --exe target/release/aicouncil`:
**RESULT: OK (version 0.2.0)** — bundle version 0.2.0 (Info.plist), arm64
(bundle + exe), codesign `--verify` clean, no obsolete GUI linkage (only
system frameworks: AppKit/WebKit/CoreGraphics/…), exe version string
`AiCouncil/0.2.0` present, **native smoke harness absent in release**
(`strings -a target/release/aicouncil | grep -c AICOUNCIL_NATIVE_SMOKE` → 0;
the debug-only module is dead-stripped; the registered `smoke_report` command
body is an inert no-op in release).

**Final artifact identifiers (03:10, pre-staging; production sources verified
unchanged since build — no file under `src/ main.rs build.rs Cargo.toml
tauri.conf.json` newer than the build; current `frontend/dist` output hash
`index-C5h9wlqI.js` matches the embedded build exactly, no rebundle needed):**

| Artifact | SHA256 |
| --- | --- |
| `target/release/aicouncil` (raw exe) | `6c2ba89b94f13b6dc8486ac12cb78ca33f32dae04df97253629d7ff81e1386c3` |
| `AiCouncil.app/Contents/MacOS/aicouncil` (signed binary) | `1882a7e5fa577a4eac074a8c5c68ac667499c0a55b181ab9d4c56a67bbb3de53` |
| canonical bundle tree hash (sorted file list) | `efaeeabe183fa28d3a0cda3646f41468c238ec5ae02285348b35f2df59c6833f` |

Signature: ad-hoc (`Signature=adhoc`, `TeamIdentifier=not set`) — **no Apple
notarization claimed**. (Bundle binary hash differs from the raw exe by
design: the ad-hoc signature is embedded into the bundled copy.)

Dist staging (dist/aicouncil + dist/AiCouncil.app + sha256 sidecars) happens
only after: (1) MM-F's native smoke gate passes on real IPC, (2) production
sources stable — sources re-verified unchanged at 03:10; if MM-F's fix touches
production code, rebuild + re-hash before staging.

## CI

`.github/workflows/ci.yml`: macOS-only (macos-14, Bun 1.4.4 pinned): frontend
install/typecheck/test/build FIRST, fmt, clippy `-D warnings`, isolated tests,
`@tauri-apps/cli` bundle, artifact upload (`if-no-files-found: error`).
**Never executed** (no push allowed) — recorded as a gap, not a pass.

## Cleanup inventory (GLM-F task-created)

- KEEP: `~/.rustup`, `~/.cargo`, `~/Library/Caches/rustup` (shared toolchain).
- Candidate: duplicate rustup toolchain `1.99.1-aarch64-apple-darwin`
  (`rustup toolchain uninstall 1.99.1`) — redundant with stable.
- Temp: `$TMPDIR/aicouncil-check.*`, `aicouncil-migrate-selfcheck.*`,
  `aicouncil-native-smoke.*` (self-cleaning on success; failure dirs kept for
  MM-F diagnosis — remove at final cleanup), `scripts/macos_clean.sh --apply`
  sweeps leftovers; `--target` removes `target/` at final cleanup only.
- Preserved by policy: `dist/data`, `dist/pc-data-backup`, `.env`, shared
  caches, `frontend/node_modules`, OpenCode sessions.
