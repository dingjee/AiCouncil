# macOS 0.2.1: UI and credential repair plan

Date: 2026-10-28. Continue `codex/tauri-macos`. User requests nine specific
repairs after the agents completed 0.2.0. This document supersedes earlier
unfinished migration work for this repair. Finish, self-check, publish locally
to dist, clean task remnants and hand back for review. Do not shut down or arm
WatchDog during this round; the user is actively reviewing the application.

## Research: observed defects and causes

1. `.app` uses three implicit grid columns while expanded children include
   sidebar, resizer, main, and sometimes a right panel. In collapsed mode the
   sidebar/resizer are unmounted, so the main element auto-places in the first
   column, whose width is explicitly zero. This explains the collapsed main UI.
2. Grid/flex minimum sizing is incomplete: main lacks a bounded minimum height,
   its implicit row can grow to message-content height, and body/app clip the
   overflow. The composer exists in JSX but falls below the window. Thread
   children also shrink. This explains missing scrolling and missing input.
3. The question wrapper clips all its children, including Expand/Collapse and
   edit/delete controls. Only the question content should be collapsed.
4. Assistant controls live in the header, ahead of the answer. Branch controls
   and a tab strip render even for a single answer. Move message actions into
   readable footer placement and use actual branching/parallel context to
   decide whether branch controls have meaning.
5. Models/Settings occupy right-hand layout columns and aggravate sizing.
   Replace them with accessible independent modal popups.
6. Theme tokens provide only a light palette; no system dark-mode behavior.
7. Existing vector icon has a transparent background and grayscale mark.
   Add a Mail-like sky-blue vertical gradient background while preserving the
   identity of the mark. Generate native icon assets from vector source.
8. Parent checked private-file metadata only: root `.env` exists and both
   OPENROUTER_API_KEY / DEEPSEEK_API_KEY are nonempty; `dist/data/.env` and
   `~/Library/Application Support/AiCouncil/.env` are absent. The current app
   looks only in its stable data directory or exe-adjacent path, and its
   legacy importer does not import `.env`. This is a delivery/path failure.
9. The earlier build is still uncommitted. Preserve it as a source baseline
   before modifications; never include private data or generated schemas.
   Earlier native send/cancel/draft fixes must remain intact.

## Rules, ownership, and model use

Use OpenCode interactive (`mini`) sessions with allowed models MM-F, GLM-F,
K4C. Three independent scopes below, no extra nested agents. Each model reads
this plan and AGENTS.md once, works its scope through actual tests, reports
concise results in its assigned status document, and waits after completion.
Do not silently abandon an assigned gate. Share exact API/asset contracts via
status files. If a provider hits quota, parent switches to another allowed
model with the existing context or a concise handoff.

NEVER read, copy, print, inspect, or run tools against real private .env files
or real database contents in external oc sessions. Use fake fixtures and mock
providers. Parent alone transfers existing private files locally with no
content output. Do not send paid chat/title requests. Never weaken tests to
obtain green results, use source-text assertions as a substitute for geometry,
or treat a running process as proof that the UI works.

Parent performs research, this plan, critical coordination, private transfer,
and a minimal independent final check. Implementers own fixes, regressions,
browser/native self-checks, and release preparation. One Cargo compiler wave
at a time; normal Cargo locks are sufficient. Avoid pgrep command-text checks
(they falsely matched themselves in the previous task).

### A — MM-F: layout, scrolling, message actions, popup UX

Own `frontend/src/App.tsx`, components (except Icon unless needed),
`frontend/src/styles/app.css`, state changes genuinely necessary for this
scope, layout/message/modal tests, browser geometry harness, and
`docs/UI_REPAIR_A.md`. Do not edit theme tokens, icons/assets, package files,
Rust or native-smoke/release scripts owned by others.

- Prefer a simple explicit flex shell or named grid areas: sidebar + resizer
  + main. Main remains flex:1/min-width:0/min-height:0 at full window height.
  Composer stays a non-shrinking footer; the thread alone scrolls vertically.
  Set bounded overflow/minimum sizing through every relevant ancestor.
- Toggle repeatedly without main width/height collapsing, scroll or draft
  loss. No right-panel layout column or obsolete right-panel CSS/state branch.
- Long questions: collapse only the body. Always-visible footer holds
  Expand/Collapse, edit/delete/copy as applicable, bottom-right inside box.
- Answers: content/status/error precede a bottom-right action footer. Copy,
  cancel, retry remain usable. Controls must not overlay or cover text.
- Single-model, unbranched conversations: hide redundant answer tab strip and
  branch-select / continue-as-main controls. Preserve needed controls for
  real parallel/branched conversations. A retry attempt alone must not make a
  normal single-model conversation look like a multi-model comparison.
- Models and Settings become modal popups, not right-hand panels. Include
  title, close button, Escape, focus trapping/restoration, accessible labeling,
  independent inner scrolling, sensible dimensions in 880x520 and 1150x680.
  Opening/closing them preserves chat width, scroll, selection and composer.
- Keep existing durable drafts, request CAS, IME handling, catalog/preferences,
  all export scopes, Markdown/math and multi-branch behavior working.

Self-check: actual browser geometry with a long synthetic imported-style chat,
not only happy-dom. Assert main width after 20 collapse/expand cycles, composer
visible and editable at multiple window sizes; real wheel changes thread
scrollTop; long-question expand increases body height while footer remains
clickable; modal content scrolls while chat geometry stays fixed; action
rectangles follow body and align to right; single vs multi-model controls
match context. Recheck relevant frontend tests/typecheck/build. Coordinate
with C for dark/light screenshots and B for native WebKit gates.

### B — GLM-F: credentials, native verification, version and release

Own `src/storage/repo.rs`, `src/storage/credentials.rs`, minimal desktop/provider
changes if necessary for credential loading, credential/migration regressions,
`src/native_smoke.rs`, scripts except icon generation, root Cargo files,
tauri.conf.json, README, final `docs/MACOS_ACCEPTANCE.md`,
`docs/UI_REPAIR_B.md`. Coordinate frontend package 0.2.1 version with C.
Do not modify real credentials/data or A/C files.

- Make bundled launch credential discovery deterministic and independent of
  working directory. Stable Application Support `.env` stays authoritative.
  Support safe first import from app-sibling `data/.env` when absent, with
  atomic non-overwrite writes and mode 0600. Preserve strict isolation with
  AICOUNCIL_DATA_DIR/AICOUNCIL_ENV_PATH so tests cannot reach private files.
- Keep .env as the sole source for both providers, no second store, no UI
  key echo, no key values in errors/logs. Report missing file, invalid fixture
  syntax, absent variable and successful reload distinctly but safely.
  Root `.env` is not a general runtime CWD fallback. Parent will place it into
  local delivery data and the stable user directory after tests.
- Test absent/valid/BOM-or-CRLF/empty/invalid .env fixtures as appropriate,
  source priority, idempotence, non-overwrite, permissions and strict isolation.
  Preserve chat DB/config unchanged during credential repair.
- Inspect earlier actual acceptance notes; reuse the working native harness
  (40 steps) and add only meaningful native checks of collapsed main geometry,
  long-history scrolling, composer visibility and question toggle if feasible.
  Check JavaScript syntax BEFORE compiling/running the harness. Use guarded
  one-time injection and bounded timeouts, capture final finished_at, and fail
  loudly with actionable diagnostics. Synthetic IME remains distinct from a
  real physical Pinyin test; never claim hardware behavior was tested.
- Bump to 0.2.1 in root Cargo/lock/Tauri metadata; C bumps frontend package.
  Wait for A/C source-stable status, build their exact final frontend, run
  strict Rust fmt/clippy/tests and final native gate once. Fix actual failures
  within ownership; coordinate others rather than edit same files in parallel.
- Required release: `cargo build --release`, package .app, ad-hoc sign, stage
  final executable and .app in dist, verify 0.2.1/arm64/codesign and embedded
  frontend identity. Do not stage an old frontend. Report separate raw and
  signed binary hashes (signing changes the bytes).
- Parent is final owner of actual private-file transfer and final system
  cleanup/commit. Report task-created temp paths/processes/caches precisely.

### C — K4C: system theme and sky-blue icon

Own `frontend/src/styles/tokens.css`, a separate new theme stylesheet/hook if
needed, `frontend/package.json`/bun.lock (0.2.1 + only necessary icon-generation
dev tooling), `assets/icons/`, icon-generation script, theme/icon verification,
`docs/UI_REPAIR_C.md`. Do not edit App/components/app.css or Rust/Tauri config.

- System light/dark preference is the default and updates live while open.
  Prefer prefers-color-scheme/CSS color-scheme for automatic behavior without
  duplicate persisted theme state. No unrequested manual override UI needed.
- Add coherent dark surfaces, text, muted labels, separators, accent/focus,
  composer, popups, code highlighting, math, selection, errors and scrollbars.
  Keep light Soft Sage identity and readable dark-mode contrast. Audit hardcoded
  light colors and override in the owned theme stylesheet where necessary.
- Ensure native titlebar uses the system theme (coordinate B if shell changes
  are required). No layout shifts or remount/draft loss on theme changes.
- Modify existing SVG source to add a rounded sky-blue gradient background
  inspired by Mail; preserve the council mark rather than replace it with an
  envelope. Generate correct high-resolution PNG and native ICNS from vector
  source. No bitmap AI tool is needed for this vector edit. Provide asset paths
  for B to use in Tauri config. Keep only consumed source/assets, no temporary
  iconsets, thumbnails or intermediate raster renders.
- Verify actual computed colors in a real browser emulating light and dark,
  switch without reload, check representative surface/text contrast and local
  Markdown/code/math readability. Coordinate A's geometry harness so both
  themes and modal states are covered. Inspect the final icon visually and
  confirm bundle uses the new icon after release.

## Final acceptance and release gates

| User item | Required evidence |
| --- | --- |
| 1 sidebar toggle | actual main rect stays wide/bounded across toggles and sizes |
| 2 scrolling/collapse | long synthetic migrated-style conversation scrolls with wheel; Expand/Collapse reachable and changes body size |
| 3 input box | existing chat always has visible editable composer; send via local native mock succeeds |
| 4 action placement | question/answer action footer follows content, bottom-right, no text overlap |
| 5 single-model simplicity | single unbranched case hides tabs/branch controls; parallel case retains them |
| 6 system theme | real-browser live light/dark switch + readable popup/code/math colors |
| 7 icon | reviewed sky-blue SVG/PNG/ICNS and bundled icon metadata/assets |
| 8 API import | fixture regressions pass; parent private file copy, mode and configured status only; no secret output |
| 9 popups | both modal dialogs keyboard/focus/close/inner-scroll work and do not resize chat |

After implementer self-checks, parent does a minimal independent spot-check:
read concise reports; inspect one final light and dark layout capture, final
artifact metadata/signature/hash/embedded source identity; verify real data DB
integrity/counts and private file equality without displaying contents. No
unnecessary full-suite repetition after successful unchanged results.

Privately place existing root .env into `dist/data/.env` and stable Application
Support .env (non-overwrite unless specifically fixing task-owned copy), mode
0600. Preserve current user conversations/config and original PC backup. Tests
must never mutate either. Confirm both named configuration fields exist without
printing values. Never embed credentials into the .app resources or Git.

Commit final source/docs on the current branch; no push. Consolidate final
current results into MACOS_ACCEPTANCE and this plan, then remove redundant
intermediate status docs. Delete target/, frontend/node_modules, frontend/dist
after bundling, gen/ generated schemas, captures, temporary browser profiles,
test databases, task-owned processes and task-created WebKit/cache/saved-state
remnants. Preserve real user data, source/lockfiles/assets/docs, shared tools
and other chats' files/processes. Screenshots never enter Git. Keep final
0.2.1 application/executable in dist. Do not shutdown during this repair round.
