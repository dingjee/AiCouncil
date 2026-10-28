# macOS acceptance report (0.2.1 UI + credential repair)

Owner: B (release/credentials/native gate). Date: 2026-10-28.
Branch: `codex/tauri-macos`. Version: **0.2.1** (Tauri 2, Apple Silicon,
`custom-protocol` embedded frontend). This report lists actual commands and
results only; superseded 0.2.0 content removed. External oc implementers
never accessed real credentials or user databases. Parent handled private local
file transfer and verification without displaying secret values or message
contents. All implementation fixtures are synthetic (`sk-fixture-*`, temp
dirs, isolated `AICOUNCIL_DATA_DIR`).

## The nine user repairs — final evidence

| # | Repair | Final evidence |
| --- | --- | --- |
| 1 | Collapsed main width 0 | Native: 20 collapse/expand cycles, every collapsed frame `main ≥ 0.8×viewport` (was 0px), final width == first (850px, no drift) — `geo:toggles_20_stable` |
| 2 | No thread scrolling | Native: real UI-path long send → thread `scrollHeight 3211 / clientHeight 451`, scrollTop adjustable away from boundaries, real `scroll` event fires — `geo:thread_scrolled`. Real wheel input separately proven by A's trusted-CDP Chrome harness (wheel 0→320) — honestly distinct methods |
| 3 | Composer missing | Native: composer wrapper flush with window bottom/right (±4px), textarea visible/editable in 1150×648 webview, still visible after every sidebar cycle — `geo:composer_visible_editable`, `toggles_20_stable` |
| 4 | Action placement | Native: question footer below body (no overlap), inside the box (not clipped), clickable before/after all geometry ops; answer footer per A's browser assertions — `geo:question_toggle_footer` |
| 5 | Single-model simplicity | A: tabs/branch controls hidden for single-model unbranched chats, retained for real parallel/branched; frontend suite + browser harness |
| 6 | System theme | C: live `prefers-color-scheme` light/dark with measured contrasts (body 11.47/13.27 etc.), no remount/draft loss, parent-reviewed captures |
| 7 | Icon | Cancelled by user override; original `logo.png`/`logo.svg` restored byte-identical, trial icns/script removed; bundle carries tauri's icns converted from the original PNG (config `["assets/icons/logo.png"]`) |
| 8 | Credential import/path | Fixture regressions green (below); parent copied real `.env` to `dist/data/.env` + Application Support (byte-equal, 0600); app now imports bundled `data/.env` deterministically |
| 9 | Models/Settings popups | A: modal dialogs with focus trap/Escape/close, no chat resize; browser-verified; parent reviewed dark Settings modal capture |

## Credentials (implemented, `src/storage/credentials.rs` + `repo.rs`)

- `.env` in the stable data dir is the sole source; allowlisted
  `OPENROUTER_API_KEY` / `DEEPSEEK_API_KEY` only; reload = re-read per lookup;
  no second store, no key values in errors/logs; no Keychain/DPAPI.
- Deterministic delivery: bundled first launch imports bundle-sibling
  `data/.env` → stable dir when absent: temp file + atomic rename,
  **non-overwrite**, **mode 0600** set explicitly, idempotent, source bytes
  and mode unchanged, path from bundle root (CWD-independent), never touches
  `credentials.dpapi`, unreachable under `AICOUNCIL_DATA_DIR` isolation.
- Robust parse: leading UTF-8 BOM stripped (dotenvy 0.15.7 does not strip it
  in the iterator path), CRLF handled (no `\r` residue), any unparsable line
  → whole file untrusted (no partial reads).
- Distinct safe diagnostics `CredentialDiagnostic` { MissingFile,
  InvalidSyntax, AbsentVariable, Present } — categories only, `Present` is
  the reload-success signal.

### Credential fixture regressions (all green, `cargo test --lib storage::` 24/24)

missing file; valid LF; BOM+CRLF (values exact, no `\r`); empty file; invalid
syntax line (distinct InvalidSyntax, value never read); absent variable;
empty value; `AICOUNCIL_ENV_PATH` > data dir priority + fallback to first
EXISTING candidate; reload lifecycle Missing→Invalid→Absent→Present→Missing;
import atomicity/0600/source-preservation; import non-overwrite; import
idempotence; import skipped without bundled `.env`; strict isolation of
candidates under `AICOUNCIL_DATA_DIR`.

## Verification gates (actual results)

| Gate | Command | Result |
| --- | --- | --- |
| fmt + clippy + full Rust suite | `scripts/macos_check.sh` (isolated data dir) | **OK** — fmt clean; clippy `--all-targets -D warnings` clean; all test binaries 0 failed (lib 105 + integration 18/9/5/5/17/4/6/12/2 …), exit 0 |
| Targeted storage tests | `cargo test --lib --locked storage::` | 24 passed / 0 failed |
| Frontend (A, unchanged evidence) | `bun test` + `tsc --noEmit` + `vite build` | 163/163, typecheck/build clean; browser geometry 19/19 (negative control: old CSS fails 11/19) |
| Theme (C, unchanged evidence) | CDP Chrome live switch | measured light/dark contrasts, no remount/layout/draft loss |
| Native gate (real WKWebView + real IPC) | `scripts/macos_native_smoke.sh` | **NATIVE_SMOKE_OK steps: 49** — full 0.2.0 contract (bootstrap 0.2.1 fixture credentials, CRUD, draft CAS, settings, catalog, send/stream/cancel, export, reload, synthetic IME gate incl. Shift+Enter, DOM render) + new `geometry_native_layout`; harness JS parsed by real parser BEFORE launch; bounded timeouts; `finished_at` present |
| Release build | `scripts/macos_build.sh` | frontend → `cargo build --release --locked` → `.app` bundle → ad-hoc re-sign → dist staged → embedded-frontend identity OK (raw exe + bundle binary) |
| Strict release check | `scripts/macos_release_check.sh` | **RESULT: OK (version 0.2.1)** — bundle 0.2.1, arm64, codesign `--verify` clean (ad-hoc, not notarized), only system frameworks, version string present, smoke harness absent (0 refs), embedded manifest verified |

Native scroll note (no overclaim): the native harness verifies overflow +
programmatic scrollTop + real scroll-event propagation in WebKit; synthetic
`dispatchEvent` wheel is untrusted and performs no default scroll in any
browser engine, so real-wheel behavior is claimed ONLY from A's trusted CDP
Chrome input (0→320) — the two methods are reported separately by design.
IME coverage remains synthetic DOM events; no physical-Pinyin claim.

## Final artifacts (dist)

| Artifact | Value |
| --- | --- |
| `dist/aicouncil` (raw exe) sha256 | `bb2e8bc7084d7c513e368a4f20f64bd87ff901427d7ba66f0262d6eb201f7764` |
| `dist/AiCouncil.app/Contents/MacOS/aicouncil` (signed) sha256 | `ee503d63050b50335ca001e829fe85f92824b6b325d61e89c7a40a3c9bea44dd` |
| Canonical bundle tree hash | `5281f4f9069f4315aeafffbab12753d7780467769005e2253d2a524479d17e6a` |
| Bundle version / arch / signature | 0.2.1 / arm64 / ad-hoc (`TeamIdentifier=not set`) — no notarization claimed |
| Bundle size | 20 MB |
| Embedded frontend | `assets/index-C2kkiMOc.js` sha256 `47755900…b5cdbcb80` == current `frontend/dist` output (A's final i18n fix included; vite content hash = identity; manifest `dist/frontend-embed.sha256`, 62 files) — no stale binary |
| Icon | `Contents/Resources/AiCouncil.icns`, converted by tauri from the restored original `assets/icons/logo.png` (`92527adf…ebcbc57c`); trial blue icon never bundled |

The raw/signed hashes differ by design (ad-hoc signature is embedded in the
bundled copy). Release-checker-only fixes after the last production build
(SIGPIPE-under-pipefail, doubled `assets/` prefix) were re-run as checks
against the existing artifacts — production sources and frontend unchanged
(no rebuild), per minimal-usage instruction.

## Credentials delivery status (parent-owned, no values shown)

- Real `.env` (both named fields) in place at `dist/data/.env` and
  `~/Library/Application Support/AiCouncil/.env` — parent verified byte
  equality + mode 0600. No manual key entry needed; the app reports
  configured status from the file at launch/reload.
- Future deliveries: shipping `data/.env` next to the `.app` is imported
  automatically on first launch (0600, non-overwrite) — no manual step.
- Live user DB preserved (non-overwrite import; new user chats kept; no
  backup-over-write path exists in code).

## Task remnant inventory (B)

- Removed by traps/verification: `$TMPDIR/aicouncil-check.*`,
  `aicouncil-b-test.*`, `aicouncil-native-smoke.*` (success-cleanup;
  failure dirs were never kept this round), `/tmp/aicouncil-harness*`.
- No B-owned processes remain (app/mock killed by driver trap; verified).
- Parent cleanup scope per plan: `target/`, `frontend/node_modules`,
  `frontend/dist`, `gen/` schemas, captures; keep dist artifacts + sources +
  lockfiles + docs.

## Parent independent acceptance and final cleanup

- Reviewed the final light collapsed-sidebar long chat and dark Settings modal
  captures; both had visible composer, readable content and correct footer
  placement. Verification captures were deleted after review.
- Ran a temporary probe against the actual compiled Rust credential loader and
  the private Application Support .env: OpenRouter configured=true, DeepSeek
  configured=true. No values were printed and no paid API call was made. The
  probe and its isolated directory were removed. Both delivered private files
  match the original source and have mode 0600.
- Checked temporary consistent copies of the user databases: original dist data
  has 3 conversations/18 messages; current Application Support data has
  4 conversations/18 messages. Both passed integrity_check and foreign_key_check.
  Current user data was preserved rather than overwritten by the old backup.
- Independently verified 0.2.1 in the bundle, Tauri config and frontend package;
  arm64, strict codesign verification, raw checksum sidecar, absence of the
  native-test harness in both deliverables, and original icon byte equality.
- Opened the final dist/AiCouncil.app for user review. No shutdown was performed.
- Final cleanup removes target/, frontend/node_modules, frontend/dist, generated
  gen/ schemas, intermediate status reports and task-owned temporary files and
  processes. Final source, lockfiles, docs, original assets, dist delivery,
  current user data, private .env files and original PC backup are retained.
