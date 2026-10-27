# Tauri macOS migration: execution and self-check plan

Date: 2026-10-28 (Asia/Bangkok). Branch: `codex/tauri-macos`.

## Outcome and constraints

Deliver AiCouncil 0.2.0 as a locally usable Tauri 2 macOS Apple Silicon app in
`dist/`, with the existing PC conversations/settings available on first launch.
Preserve the Soft Sage visual identity and existing product behavior. macOS is
the sole supported platform. No paid service calls are necessary for acceptance.
Use the user's OpenCode models for implementation, in interactive sessions.
Codex supplies the plan, resolves integration blockers, and performs a small final
independent acceptance. Finish inside the available five-hour usage window;
target implementation and first release within three hours, leaving a buffer.

Never read API keys into model context, logs, screenshots, source, or commits.
Never run tests against the real user database or delete unrelated machine data.
Keep existing uncommitted changes recoverable in a baseline commit on this new
branch before replacing the UI. Do not push unless separately requested.

## Research findings

- Current version: 0.1.10; UI is iced 0.14 with patched tiny-skia. Domain, SQLite,
  OpenRouter/SSE, request lifecycle, recovery, title and export services are
  reusable independently of the visual layer.
- The source tree contains substantial uncommitted Windows development changes.
  Preserve their full source before removing obsolete implementation files.
- This host is macOS 27.2, arm64; Xcode CLI tools and Bun 1.4.4 are installed.
  Rust/cargo are absent from PATH. The old pinned Rust version is 1.99.1; verify
  availability and install a supported stable toolchain rather than loop on an
  unavailable pin. Avoid installing unnecessary global tooling.
- `dist/data/aicouncil.db` passes SQLite quick_check. Baseline counts: 3
  conversations, 18 messages, 12 generations, 12 usage records, 2 groups, 6 schema
  migrations. Compare actual IDs/content/config through hashes or code, without
  printing private content. No WAL/SHM files are currently present.
- PC data includes config.json and credentials.dpapi. Windows DPAPI ciphertext
  cannot simply become a Mac keychain entry; preserve the original encrypted
  file and explain credential re-entry. Never claim keys were migrated unless
  a legitimate decryptable source is actually available.
- Default current data path is executable-relative. A .app bundle must not write
  its runtime database inside Contents/MacOS. Choose a stable, documented Mac
  location, support AICOUNCIL_DATA_DIR for isolated testing, and safely import
  legacy sibling `data/` without overwriting an existing destination.
- Official Tauri documentation: https://v2.tauri.app/start/prerequisites/ and
  https://v2.tauri.app/distribute/macos-application-bundle/ . Verify current test
  support before choosing a native harness; a mocked browser UI alone does not
  establish that native IPC works.
- oc model IDs available: `kimi-code-plan-global/kimi-for-coding`,
  `minimax-cn-coding-plan/MiniMax-M3.1-Flash`,
  `zhipuai-coding-plan/glm-5.3-flash`.

## Phase 0: preserve and establish contracts

1. Create the new branch, preserve all pre-existing source changes in a baseline
   commit. Exclude dist, .env, DB/credentials, screenshots, caches and temp files.
2. Make a byte-identical user-data backup under ignored `dist/`, record hashes
   and table counts in a non-sensitive migration report. Do not change originals
   while implementation/tests are running.
3. Lead implementer writes `docs/TAURI_IPC.md` before the frontend depends on it:
   exact command names, argument nesting/casing, DTOs, null/error conventions,
   event payloads and cancellation/retry semantics. Favor a small coherent API
   with full state snapshots and incremental events; never expose keys in state.
4. Establish non-overlapping file ownership below. Contract changes must be
   written to the shared document and communicated before consumers diverge.

## Parallel work and ownership

### K4C: architecture, Rust/Tauri integration, final integration

Own Cargo.toml/Cargo.lock, build.rs, src/main.rs, src/lib.rs, new desktop/IPC
modules, tauri.conf.json/capabilities, docs/TAURI_IPC.md, integration of old
app-independent draft logic, necessary domain/provider/service compatibility.
Own removal of iced-specific code/vendor/assets/tests once replacements work.
Do not edit frontend files or storage credentials/path modules while others own
them. Reuse existing service logic; do not reimplement networking in JavaScript.

Required backend operations: bootstrap/state, conversations/groups CRUD,
selection/main-branch changes, model catalog/preferences/primary/overrides,
settings/system prompt, durable drafts/attachments/latest-question editing,
send/cancel/retry/delete, usage/cost view, Markdown export, credential save/delete
with only a configured-status boolean returned. Persist all state through
existing storage APIs and preserve admission/egress/lineage/idempotency rules.
Keep the service alive through conversation switches; route events by IDs.
Build a real Tauri window with CSP and minimal capabilities, native menus and
Cmd shortcuts. Avoid shipping development servers, inspectors or test modes.

### MM-F: frontend

Own frontend source, package manifest/lock, frontend build configuration and
frontend unit/UI tests. Read the IPC contract and use its exact command names.
Prefer a modest maintained stack and local bundled rendering dependencies.
Never fetch Markdown/LaTeX libraries/fonts from a CDN at app runtime.

Implement sidebar chats/groups, search, rename/delete/move; model picker with
search/family grouping/enabled/primary/multiple selection; settings and key
entry; main chat with streaming Markdown/code/math/CJK, selection/copy,
parallel answer tabs and split view; branch continuation/multi-branch followups;
composer with text attachments, long-paste conversion and limits; durable
per-chat drafts, latest-question edit and message removal; cancel/retry/errors;
usage labels and export. Preserve original English UI/Soft Sage default, macOS
titlebar spacing, keyboard navigation and readable resizing.
IME composition Enter must not send. Shift+Enter inserts newline; Cmd+Enter and
normal Enter follow clear documented behavior. Prevent stale async responses
from switching chat/draft state. Do not render unsanitized HTML from models.
No decorative dead buttons or silent stubs; implement, remove, or report gaps.

### GLM-F: environment, data migration, credentials, verification/release tooling

Own src/storage/repo.rs, src/storage/credentials.rs, new migration-focused tests,
scripts/*.sh or *.py for repeatable build/check/cleanup, macOS CI,
README.md/.env.example/.gitignore and docs/MACOS_ACCEPTANCE.md. Own temporary
tool installation coordination; communicate actual cargo/node/bun paths.
Install only necessary toolchains and keep an inventory of created directories,
process IDs, browser downloads, caches and macOS app/Keychain test identities.

Use macOS Keychain for persistent credentials; isolate tests from the real
Keychain or use unique disposable test identities that are deleted afterwards.
Support isolated temp data paths, safe SQLite backup/import including WAL,
non-overwrite/idempotence and stable bundle-independent runtime data location.
Report DPAPI re-entry explicitly and preserve the ciphertext backup.
Do not modify the real DB until final controlled migration. Coordinate root
Cargo dependency needs with K4C rather than editing the same file concurrently.
Prepare executable release/check scripts and a detailed actual-results report;
do not label a check passed based only on code inspection.

## Verification gates

1. Install prerequisites while source work proceeds. One shared Rust build at a
   time; avoid redundant release builds and multiple dependency trees.
2. Preserve meaningful domain/storage/provider/service tests. Remove only tests
   that exclusively exercise the removed renderer; replace critical behavior
   coverage in the new UI/backend tests. Format, clippy with warnings denied,
   Rust tests, frontend typecheck/build/tests must pass.
3. Run local mock-based integration for send/stream/complete, parallel models,
   multi-branch context, cancel, retry/failure, request recovery, egress rejection,
   draft persistence/consumption races, title CAS, usage deduplication and export.
   Assert actual requests/DB side effects, not only a mocked UI expectation.
4. Test migration with copies of the legacy DB/config, old schema and WAL cases;
   verify integrity_check, foreign_key_check, IDs/counts/content equivalence,
   groups, preferences, drafts and monetary precision. Second startup/import
   must not duplicate or overwrite records.
5. Exercise the new frontend with real IPC or an appropriate native harness:
   initial load, existing chat, new chat/draft across restarts, picker/settings,
   send/stream/switch/cancel/retry, tabs/split/branches, attachment limits,
   Markdown/LaTeX/code/CJK selection, export and resizing. Browser-level tests
   supplement native checks. Test scripts must not touch actual user data.
6. Build 0.2.0 using `cargo build --release` as required by AGENTS.md, then create
   the .app with the same final sources. Ad-hoc sign locally where appropriate;
   no claim of Apple notarization without certificates. Copy final app and
   executable to dist, hash final executable, verify bundle version/architecture,
   codesign structure and dependency absence of iced. Launch from dist on an
   isolated data path; check actual process/window and IPC startup result.
7. Controlled final user migration: consistent backup, then import/copy original
   PC database/config to documented runtime location or packaged portable data
   as designed. Verify complete row/content preservation before opening it.
   The final dist artifact must open the migrated data without manual file moves.
   Preserve one clearly labeled original backup, not scattered intermediate DBs.
8. Codex final minimal independent gate: read report, spot-check IPC contract
   against UI/backend, verify final dist metadata/hash, migration integrity and
   absence of leftover task processes/files. Request focused fixes only for real
   gaps; avoid re-running already proven full suites without a source change.

## Final cleanup, version record, shutdown

Create a concise durable acceptance report listing actual commands/results,
counts, artifact hashes, data location, credentials caveat and any unverified
manual input/signing limits. Update current documentation rather than retain
irrelevant Windows execution instructions. Commit final source/docs on the new
branch; keep private runtime data and binaries ignored, with final delivery in
dist. Do not push. Remove obsolete Windows exe/scripts, iced vendor/rendering
and intermediate probes after dependency/usage checks; historical source stays
recoverable through git.

Delete target/, node_modules, test results, screenshots, temp logs, downloaded
archives, temporary DBs and frontend staging output duplicated inside the final
app. Keep authored source/config/lockfiles/docs/assets, required root build
metadata, private .env/user data and final dist. Keep only final required frontend
assets if Rust release embedding needs them; explain reproducible rebuild flow.
Stop only task-owned mock/dev/browser/app processes. Clean only task-created
system temp/cache/app data and disposable Keychain entries, preserving pre-existing
user caches, OpenCode sessions, shared tools and unrelated running applications.
Use a recorded inventory to avoid unsafe broad cache deletion.

After successful delivery and cleanup, perform the user's authorized full macOS
shutdown. sudo currently requires a password; determine a legitimate no-prompt
system shutdown route early. Do not create passwordless sudo rules, store admin
passwords, force-kill unrelated apps or misreport sleep as shutdown. If macOS
authorization prevents shutdown, record the exact blocker and report it candidly.
