# Implementation Status

Recorded by phase: completed items, verification commands/results, unresolved items, next steps.

## P0 Engineering Skeleton and Rendering Validation — ✅ Complete (2026-10-17)

### Completed Items

- P0.1 Cargo package (aicouncil, lib + bin), rust-toolchain.toml pinned to
  1.99.1, dependencies locked (Cargo.lock committed), minimal CI
  (.github/workflows/ci.yml, fmt/clippy/test/release for ubuntu+windows; not
  actually run on GitHub Actions yet — this repo has not been pushed to a
  remote, so verified with locally equivalent commands).
- P0.2 `examples/render_probe.rs` (real native window, no network) +
  `tests/fixtures/render_cases.md` (embedded load).
- P0.3 Validated mixed Chinese-English text / code highlighting / tables /
  links / scrolling / zoom / font fallback (visual check in a real window);
  text selection is an inherent limitation of iced 0.14;
  the message-level Copy button landed in P3 (see known gaps in
  docs/RENDERING.md).
- P0.4 All four delimiter classes, fractions/integrals/radicals/series/
  matrices/cases/aligned (including the align→aligned mapping) render as
  graphics; automated assertions in `tests/math_coverage.rs`; `$` never
  reaches an uncontrolled TeX executor (iced_math is pure Rust, no JS).
- P0.5 Render caching: formula SVG cache key = (source, font size, mode,
  color); text atomic measurement cache key = (font style, text), intrinsic
  width independent of container width; formula-free paragraphs take the
  rich_text fast path, streaming rebuilds only trailing paragraphs.
- P0.6 docs/RENDERING.md records the version, architecture, validation, and
  gaps.

### Verification Commands and Results (local Windows 10 19045 / Rust 1.99.1)

```sh
cargo check --all-targets        # passed, 0 warnings
cargo test --lib                 # 27 passed; 0 failed
cargo test --test math_coverage  # 3 passed (including the \LaTeX fallback assertion)
cargo run --example render_probe # ran in a real window, verified visually (see docs)
```

### Unresolved Items

- CI has configuration only; never run remotely (no remote repository).
- iced 0.14 has no text selection; known gaps documented in
  docs/RENDERING.md.

## P1 Domain Model, Storage, and Send Planning — ✅ Complete (2026-10-17)

### Completed Items

- P1.1 Domain entities (ids/tree/plan/money), initial SQLite migrations
  (foreign keys, WAL, indexes), platform data directory, temp-file database
  tests.
- P1.2 Planner (single branch / same-prompt multi-model / multi-branch
  follow-up) plus branch isolation, duplicate-branch rejection, verbatim
  retry, and empty-prompt rejection tests.
- P1.3 Config file (non-sensitive, atomic writes) + credentials (keyring
  first, in-process fallback, probe caching, never stored in plaintext).
- P1.4 Batches persisted to the database in a single transaction (all-or-
  nothing rollback on failure), tree reload by topology, startup recovery
  (Queued/Connecting/Streaming → Interrupted, idempotent), independent usage
  states (Pending/Known/Unavailable; idempotent crediting that never rolls
  back and never double-bills).

### Verification

```sh
cargo test --lib             # 62 passed (including tree/plan/money/credentials/catalog)
cargo test --test p1_lifecycle  # 5 passed (full branch lifecycle/recovery/rollback/S2)
```

## P2 OpenRouter, Model Catalog, and Streaming Scheduling — ✅ Complete (2026-10-17)

### Completed Items

- P2.1/P2.2 OpenRouter adapter: `/models`, `/chat/completions` (SSE),
  `/generations` follow-up queries, `/key` status; `/v1` deduplicated; error
  classification (401/402/429/model/context/network/timeout); cancellation
  tokens; all monetary amounts are Decimal (parsed from string literals, no
  f64 intermediate noise).
- P2.3 Catalog cache (24h TTL, concurrent refreshes coalesced, stale data
  kept on failure), preference separation, Auto/Override parameter synthesis
  (optional parameters never fabricated; out-of-range overrides rejected and
  reported), effective output cap (model limit × context headroom).
- P2.4 ChatService: semaphore=4, cancellable queueing, persisted to the
  database before the request is sent, 300ms/4KiB throttled persistence,
  final states persisted only once, late-arriving usage credited and
  forwarded to the UI.
- P2.5 Local TCP mock (7-byte chunk splits + inter-chunk delays): stream
  isolation, UTF-8 across chunk boundaries, event boundaries, cancellations
  independent of each other (partial text preserved), 401 classification,
  duplicate final states, catalog refresh preserving preferences.

### Verification

```sh
cargo test --test p2_mock_stream  # 6 passed
cargo clippy --all-targets -- -D warnings  # zero warnings
```

### Unresolved Items

- In the interleaving test both requests share the same mock (ChatService has
  a single provider); cross-provider parallelism (different base_url) to be
  verified later during P4 multi-model UI integration.
- Live integration against real OpenRouter pending a user-supplied key
  (everything currently validated against mocks).

## P3 Minimal Native UI — 🚧 Skeleton Complete (2026-10-17)

### Completed Items

- **Renderer decision**: wgpu produced no frames in the local remote desktop
  session (view calls fine, no panic, screen all white;
  `WGPU_BACKEND=dx12/vulkan/gl` all identical), switched to **tiny-skia
  software rendering** (iced default-features=false + tiny-skia); rendering
  verified fully correct. Performance is sufficient for a document-style UI;
  on real GPU hardware the wgpu feature can be restored in Cargo.toml.
- P3.1 Main area (header: Export MD / All branches / collapse sidebar;
  scrolling message stream; bottom input area: model picker / multi-line
  editor / Send) + right sidebar (three tabs: sessions/models/settings,
  collapsible) — layout verified correct in a real window (visual check).
- P3.2 New chat / search box / single-level grouping (ungrouped + custom
  groups) / group rename and delete / session rename and delete / move to
  group; UI wired to DB persistence.
- P3.3 Models panel: API key input (secure, saved to the credential store or
  in-process), Test connection, catalog refresh (cache kept on failure),
  model catalog check / set default / reorder, model search.
- P3.4-3.5 (partial) Send pipeline wired up: Planner → ChatService.submit →
  broadcast events → UI updates by ID; scroll following (mouse_area
  detection).
- P3.6 Export service (export.rs): Main/Selected/AllBranches scopes, atomic
  writes, filename sanitization; unit tests pass; UI Export button wired
  (file dialog).
- Startup recovery: leftover non-final states → Interrupted (S1); current
  session UI state persisted.

### Verification

```sh
cargo test               # 80 passed (66 lib + 3 math + 5 p1 + 6 p2)
cargo clippy --all-targets -D warnings  # zero warnings
cargo run                # real window: layout/sidebar/input area render correctly (visual check)
```

### Unresolved Items (P3 wrap-up)

- GUI automation click coordinates are hard to land reliably under DPI
  scaling; the send → stream → render **end-to-end manual acceptance** is
  still pending (needs a real key or a mock base_url injection setting).
- Enter confirming a Chinese IME candidate must not trigger a send (P1
  acceptance matrix); pending verification on real hardware.
- pick_list rendering under tiny-skia still to be confirmed.

## P3 Single-Branch Closed Loop — ✅ Complete (2026-10-18)

- `AICOUNCIL_BASE_URL` injects the local mock; mock_openrouter example
  (/models + SSE streaming + /key + costs).
- Send pipeline end to end: input → Planner → single-transaction persistence
  → SSE streaming → 300ms/4KiB throttled persistence → events routed by the
  4-tuple → card rendering → on completion main_leaf points at the primary
  answer.
- Sidebar: current session highlight / rename / delete / move to group;
  input area: model picker.
- Verified end to end in a real window (visual check).

## P4 Parallel and Branching — ✅ Complete (2026-10-18)

- Same-prompt multi-model: checking "Parallel" sends to all enabled models
  with the primary model first; parallel answers shown as side-by-side cards
  (mock/beta + mock/alpha, visual check).
- Multi-branch follow-up: the completed card's "Branch" checkbox →
  MultiBranch send (each branch uses its current model; Cartesian product
  forbidden); multi-selection cleared after sending.
- "Continue from here": switches the main branch to that completed answer;
  rejected when not completed (B3).
- Two-stage E2E (AICOUNCIL_E2E=multi2): 4 POSTs; the DB tree verifies both
  branches each contain follow-up user nodes with identical content but
  different IDs (B2 isolation); main_leaf points at the main branch's
  second-round answer.
- Independent stop/retry: per-card granularity, independent cancellation
  tokens.

## P5 Costs — ✅ Core Complete (2026-10-18)

- Cost label: bottom-right of the input box, "Known cost $x · pending N ·
  unavailable M"; exact Decimal accumulation (mock two calls
  $0.00042×2=$0.00084, verified in the UI); usage cache refreshed
  via refresh_from_db.
- Already in place: idempotent usage event crediting, deduplication of
  duplicate reports, the Pending/Known/Unavailable state machine, and restart
  recovery of pending-lookup records (`pending_usage`).
- To be refined (does not block acceptance): the 300ms-debounced estimate and
  its display, wiring the 2/5/15s generation follow-up query schedule into
  the UI, and an expandable per-request detail panel.

## P6 Delivery — ✅ Windows Complete (2026-10-18)

- Release build passes; the release binary verified via mock E2E (send →
  stream → persist).
- README (running / data locations / credentials / offline acceptance / known
  limitations).
- Acceptance matrix conclusions: docs/ACCEPTANCE.md (R1-R3/B1-B4/S1-S2/
  M1-M2/C1-C2/E1 pass; P1 partial; of the three platforms only Windows
  verified).
- CI configuration ready (never run remotely); no certificate for code
  signing, marked incomplete.

## 0.1.1 Fixes (minimal UI / portability / icons) — ✅ Windows Complete (2026-10-18)

Plan and item-by-item verification: after verification was completed, the
internal plan document was moved out of the repository during cleanup (not
distributed with the repository).

### Changes

- Startup: `src/main.rs` declares the Windows GUI subsystem (double-click
  opens no CLI window; PE subsystem=2 verified on the release exe); when the
  data directory is unwritable, an rfd visible error dialog is shown instead
  of silently falling back to the CWD.
- Data/credentials: default root directory `current_exe().parent()/data`;
  legacy ProjectDirs data migrated via the SQLite backup API (WAL-consistent
  snapshot; tests cover un-checkpointed records with the source database
  unchanged), existing local databases never overwritten; Windows
  credentials DPAPI-encrypted into `data\credentials.dpapi` (imported once
  from the old keyring), valid for the current run only on other platforms;
  all documented in the README.
- UI: message headers / input area switched to small icons + tooltips
  (`src/app/icons.rs`, Lucide SVG, license bundled); API key moved into
  Settings; the models panel keeps only search/checkboxes, prices tooltip
  only; the cost entry consolidated into a "Usage" expansion (known / pending
  lookup / unavailable + key quota); empty state centered; group rename fixed
  (RenameGroupApply now actually persists to the database).
- Icons: `logo.svg` viewBox cropped to the subject; logo.png and the 7-size
  ICO regenerated (16-256, subject occupying roughly 90-95% of the canvas);
  `build.rs` tracks the three icon inputs and panics outright if an icon is
  missing or embedding fails.
- Window: new minimum size 880×520 logical pixels — below that width the
  sidebar (300) plus input toolbar pushes the Send button out of the visible
  area (found during acceptance and fixed in a targeted way).

### Check Results

```sh
cargo fmt --all -- --check                 # passed
cargo clippy --all-targets -- -D warnings  # zero warnings
cargo test                                 # 84 passed (70 lib + 3 math + 5 p1 + 6 p2)
cargo build --release                      # passed; subsystem=2, icon resources present
```

### GUI Evidence (all from the final delivery exe, isolated data directory, mock content; captures taken locally, not kept in the repo)

- Empty-state main window + Settings + Usage expansion (data directory shown
  as a data folder next to the exe)
- Dual-model parallel cards
- Models panel (no persistent prices/quota)
- Minimum-size window: Send button visible, card headers wrap at component
  level
- Old vs new icons at 16/32/256

### Known Observations and Unverified Items

- In the local remote desktop session, the first frame of a freshly launched
  window occasionally leaves the bottom region uncomposited (visible via
  PrintWindow capture); moving the window or a subsequent repaint restores
  it — to be observed on real hardware, not a logic defect.
- macOS/Linux, real-hardware IME, and live OpenRouter integration remain
  unverified (see docs/ACCEPTANCE.md).

## P7 Attachments and Parallel Multi-Select — ✅ Windows Complete (2026-10-18, 0.1.2)

### P7B Text Attachments + Long Paste to Attachment

- Domain layer: `src/domain/attachment.rs` — `Attachment{name,content}`,
  `compose_prompt` (format
  `{prompt}\n\n===== 附件 N: name =====\n{content}`; plain-text
  concatenation, does not touch the OpenRouter multimodal format, transparent
  to all models), and pure limit functions (single file ≤256KB / ≤10 files /
  1MB total; over-limit skipped + toast).
- `Message.attachments` / `Conversation.draft_attachments` fields; migration
  `0002_attachments.sql` (JSON column added to both messages/conversations;
  old databases auto-migrated via `storage::open`; tests cover the 0.1.1
  legacy database path).
- Send pipeline: `SendPlan.attachments`; `Planner::prepare` relaxed so a send
  is invalid only when both prompt and attachments are empty;
  `build_history` / `export::history_of` compose attachments into historical
  user messages (visible in follow-up round context);
  `ImmutableRequest::from_planned` composes the final prompt; retries rebuilt
  via `compose_prompt(user.content, attachments)`; each user node of a
  multi-branch follow-up automatically carries the same attachments.
- UI: "Attach" button in the input area (rfd multi-select, common text/code
  extensions + all files; `read_to_string` naturally validates UTF-8);
  pasting ≥2500 characters (constant, matching Claude.ai) auto-converts to an
  attachment; chip bar between editor and toolbar (name/size/remove); chips
  saved/restored with the session draft and cleared after sending; read-only
  attachment chips below the user message card; export appends
  `[附件 N: name]` + full text after the user text.

### P7A Parallel Temporary Multi-Select

- `parallel_selection: Option<Vec<String>>` (None = follow the models panel's
  enabled set, the default; Some = the temporary checked set for this send);
  pure function `resolve_parallel_targets` (None → all enabled; Some →
  filtered intersection with the enabled set, order preserved; primary model
  forced first when present in the result; empty = invalid send + toast).
- UI: after checking "Parallel" an "N models" entry appears; the expanded
  panel offers per-model checkboxes ("Primary" badge) + "Select all / follow
  panel"; disabling a model in the models panel automatically removes it from
  the temporary selection; existing E2E paths such as `AICOUNCIL_E2E=multi`
  are unaffected (default None).

### Check Results and Evidence

```sh
cargo fmt --all -- --check                 # passed
cargo clippy --all-targets -- -D warnings  # zero warnings
cargo test                                 # 100 passed (85 lib + 3 math + 5 p1 + 7 p2)
cargo build --release                      # passed; exe copied to dist/
```

- New tests: compose_prompt/limits (5), resolve_parallel_targets (3),
  paste-threshold classification, attachment storage roundtrip and the 0002
  migration (2), attachment export, and the E2E
  `attachments_composed_into_request_prompt` (captures the mock request body
  and asserts it contains the attachment separator and content, and that the
  user node stores the original prompt + attachment fields).
- GUI visual checks (final delivery exe, isolated data directory):
  attachment chip bar; parallel multi-select panel expanded with the
  "Primary" badge (captures taken locally, not kept in the repo).

## Next Steps

- Live integration against real OpenRouter (after the user supplies a key).
- P5 items to refine (estimate debounce / follow-up query scheduling / detail
  panel).
- macOS/Linux verification on real hardware; IME acceptance on real hardware.


## 0.1.3 UI Foundation Mechanism Overhaul — ✅ Complete (2026-10-19)

Implemented per `docs/UI_FOUNDATION_PLAN.md` (11 requirements consolidated
into four foundation mechanisms). **That plan file was removed after
completion**; this section, together with "Selectable Document and Copy
Projection (2026-10-19)" in docs/RENDERING.md and "0.1.3 acceptance" in
docs/ACCEPTANCE.md, is the primary record of process and feedback for
follow-up investigation to pick up.

### Deliverables (numbered per the plan)

- **1 Input area / 5.1 Composer**: stable `text_editor::Content` (rebuilt
  only on session switch / after send; no longer reset per-frame with
  with_text); min 76px / max `min(220, 0.30×window height)`; explicit line
  height 1.55, Word wrap, dedicated widget id. The duplicated-character root
  cause was not reproduced on-site (plan G0-E not executed); dynamic height +
  explicit parameters are defensive hardening.
- **2 Model selection**: removed the old "pick_list + persistent panel" dual
  entry; added `ModelSelectionState` (parallel/checked/primary) + the pure
  function `resolve_selection` + `default_parallel_pair` (current + most
  used; panel order when there is no history); the Send-disabled reason, cost
  estimate, and SendPlan all share `NormalizedSelection`; the primary is
  explicitly designated with a star; the parallel toggle is disabled in
  multi-branch follow-up mode.
- **3 Scrolling**: `app::scroll::ScrollController` (Following/Reading/
  Selecting, 24/48px hysteresis, suppress counter for programmatic scrolls,
  at most one action per round, session anchors kept in memory);
  `dirty_stream` changed to a HashSet — one tick refreshes all dirty
  messages, then scrolling is computed once; scrollable `on_scroll(Viewport)`
  judges by actual viewport distance, deleting the old dy-guessing logic.
- **4 Collapsing**: long user messages collapse (>320 characters or >8
  lines; display layer `clip(true)` with fixed height + bottom chevron,
  Expand/Collapse question tooltip; per-conversation expanded set held in
  memory). Actual layout-height measurement not done (heuristic; known
  limitation).
- **5 Font size**: φ type scale (2.618/2.058/1.618/1.272/1.128/1) + body
  12–22 slider persisted; heading/body line-height and block-spacing rules
  implemented in the view layer.
- **6 Tables**: empty cells preserved + missing columns padded; row height
  content-driven; 1px gap lines; light-tinted header; alignment applies to
  text.
- **7 Lenient Markdown**: `ParsePolicy::{Strict, LlmCompatible}`; conservative
  added recognition of Chinese-punctuation `**…**` (same part only, only
  empirically tested combinations, bounded complexity, differential checks
  against ambiguous counterexamples).
- **8 Auto-titling**: `migrations/0003_title_metadata.sql` (title_source/
  status/revision/attempt_count + an auxiliary_requests ledger with unique
  constraint (conversation,purpose,turn)); field-level CAS
  (`rename_conversation_title` / `apply_generated_title` /
  `set_fallback_title`; draft/leaf updates no longer carry the old title for
  full overwrite); `providers/deepseek.rs` (explicit env > same-named `.env`
  variable, injectable endpoint, timeout 12s, thinking disabled, 80 tokens,
  response sanitized to ≤48 graphemes, multi-segment/quoted/empty responses
  rejected) + `services/title.rs` (single in-flight per session, cross-
  session semaphore of 2, triggered only after successful persistence,
  retried on the next question after a failure, no automatic consecutive
  retries, events routed by ConversationId). Real API not yet exercised.
- **9 Sidebar width**: divider (mouse_area 6px hit zone / 1px visual /
  horizontal resize cursor / double-click resets to 300 / one-time write to
  disk on drag end); config `sidebar_width` clamped to 240–480 and bounded
  by window width; invalid/NaN falls back to the default.
- **10 Icons/tooltips/English/rounded corners**: `ui_style.rs` semantic
  tokens (full Soft Sage token set) + `design.rs` control styles (quiet/
  accent/active/danger buttons, text_input focus ring, cards/chips/sidebar/
  composer/divider); `i18n.rs` full English resource table (about 130 keys,
  missing keys fall back to the key itself, single entry point for
  interpolation/plurals); icons switched to token-based colors + new
  edit/star icons; all action buttons are icon + English tooltip.
- **11 Local selection**: `render/document.rs` (stable PathSeg paths +
  FlatDoc leaf flattening + projection), `render/flow.rs` (MathFlow selection
  surface: hit_test/grapheme positioning/selection highlighting/double-click/
  triple-click/formula atoms indivisible), `render/code_flow.rs` (code
  (line,col) selection), and an app-level SelectionCoordinator (within one
  message across blocks; Ctrl+C focus arbitration; formula copy yields the
  raw source; clipboard untouched when there is no selection). Cross-message
  drag selection not done (known limitation).

### Issues Found and Fixed During Smoke Testing

- **Sidebar invisible on first launch** (found before the 0.1.3 release):
  the sanitized `sidebar_width` local variable in boot was never written back
  to `config.window.sidebar_width`; a fresh config defaulted to 0.0 →
  `Length::Fixed(0)`, hiding it. Fix: sanitize and write back directly in
  boot (invalid/NaN/out of range → 300). Lesson: sanitized results must land
  on the field actually consumed, not on a parallel local variable.
- **Sidebar invisible in screenshots under the current RDP session**
  (environment issue; code proven correct): in the local RDP session the
  Display-virtualized DPI differs from the System DPI (96); an iced 0.14
  layout-tree dump (Operation traversal) confirmed root=1150×680,
  main=844×680, and the sidebar node present at 850..1150 — i.e. **layout
  correct**. But on-screen pixel sampling shows main-area content drawn past
  the 844 boundary filling the whole window, contradicting the layout tree —
  a scaling/clipping mismatch between the tiny-skia frame and the logical
  layout. Controlled experiment: a [Fixed,Fill] row renders correctly; a
  [Fill,Fixed…] row is invisible in this session (reproducible with plain
  text). The session at 0.1.2 delivery time (150% DPI) showed no such
  symptom. Disposition: keep the standard row structure (the same [Fill,
  Fixed] shape as 0.1.2) and re-run acceptance (V01–V11) in a real-monitor
  session; screenshots from this session are not used as UI acceptance
  evidence (palette/copy/startup smoke remain valid).
- Isolated-data-directory launch verification: process alive, window title
  normal, Soft Sage palette and English UI in effect (in the screenshots the
  main area and empty state render correctly; the sidebar portion is affected
  by the environment issue above and is not used as layout evidence).

### Version and Checks

- **Sidebar invisible on first launch** (found before the 0.1.3 release):
  the sanitized `sidebar_width` local variable in boot was never written back
  to `config.window.sidebar_width`; a fresh config defaulted to 0.0 →
  `Length::Fixed(0)`, hiding it. Fix: sanitize and write back directly in
  boot (invalid/NaN/out of range → 300). Lesson: sanitized results must land
  on the field actually consumed, not on a parallel local variable.
- Isolated-data-directory launch verification: process alive, window title
  normal, Soft Sage palette and English UI in effect (visual check,
  re-verified after the fix).

### Version and Checks

- Version 0.1.2 → **0.1.3**; after `cargo build --release`, copied to
  `dist/aicouncil.exe` (SHA256 in the ACCEPTANCE release record).
- `cargo fmt --all -- --check` / `clippy --all-targets -D warnings` /
  `cargo test`: all green (146 tests).

### Note on Removing the Plan File

`docs/UI_FOUNDATION_PLAN.md` was a one-off implementation plan (its Section 0
states explicitly that "this round's delivery is the plan"); per the user's
instruction it was removed after completion. The necessary process/feedback
was distilled into:
- docs/RENDERING.md → rendering architecture changes + lenient parsing +
  tables + type scale + new gaps
- docs/ACCEPTANCE.md → T-numbered automated results + unverified list
- docs/IMPLEMENTATION_STATUS.md → this section (deliverable mapping/
  decisions/limitations)
- README.md → user-visible behavior (model picker / auto-titling / .env /
  font size / sidebar / English UI)

### Known Limitations and Follow-up Suggestions (pick up from here)

1. V01–V11 native UI acceptance not executed (on-site reproduction of the
   duplicated-character input issue is top priority).
2. Cross-message drag selection, collapsing by actual layout height, Reading
   node-level anchors, and random-chunking fuzz (T10) are follow-up work at
   the rendering layer.
3. Real DeepSeek integration for titles (once `.env` is provided); the usage
   detail panel currently shows summary counts only (the plan's minimal
   "flag title auxiliary calls" implementation).
4. The vertical-stacking logic for parallel cards in a narrow main area is a
   placeholder (the window size is known but the layout branch never takes
   effect — still side by side; need to switch to column at the card row
   based on available width).

---

## 0.1.4 (2026-10-19): Egress Gate / Request Recovery / Family-Grouped Picker

Completed per an internal implementation plan (the plan document has served
its purpose and is not distributed with the repo; for the behavioral contract
see the ACCEPTANCE 0.1.4 acceptance matrix).

### New Modules

- `src/domain/model_identity.rs`: vendor attribution (the Anthropic
  determination covers all `anthropic/*` and curated `claude-*` aliases,
  unaffected by display names) + family-grouped presentation
  (`presentation`: group title/short name/full name; variants like `:free`
  preserved).
- `src/domain/recovery.rs`: FailureInfo (versioned JSON, graceful degradation
  on unknown), DispatchPhase, RetryDecision (eligibility/cooldown/
  remediation), backoff 2/4/8…30s.
- `src/services/egress.rs`: GeoIP egress gate (ip.sb primary → ipapi.co
  fallback; CN/HK blocked, other valid countries allowed, Unknown blocked;
  64KiB cap, UA, 8s budget; permit TTL 3s + network_revision).
- `src/services/network.rs`: unified network configuration (generation:
  connect 15s / read-stall 120s; detection shares the generation
  configuration; explicit proxy `AICOUNCIL_PROXY`; failures never silently
  fall back to direct connections).
- `src/app/model_picker.rs`: grouped view model (group order = first
  appearance among enabled; search matches family/short name/full name/
  original name/raw ID).
- `migrations/0004_request_recovery.sql`: failure_json /
  request_snapshot_json / retry_root_id / retry_not_before_ms /
  dispatch_phase + unique-active partial index on lineage + root backfill.
- `scripts/winshot.ps1`: window screenshot tool (auto-moves to the secondary
  display).

### Behavior Changes (highlights)

- Send flow: preflight (admission) → single-transaction persistence
  (including the intended main_leaf CAS) → dispatch (re-check when the
  permit has expired) → final-state CAS; receipts reconciled against the
  DB's authoritative state. Preflight/persistence failures roll back nothing
  (no tree insert / no draft clearing / no leaf changes).
- Failure recovery: a final-state failure does not block new rounds; an
  empty failure placeholder is excluded from model context while partial
  output is retained; EOF without a successful final state is judged a
  failure (no longer conservatively written as Completed); error frames
  (top-level error / choice.error / finish_reason=error) always count as
  failure.
- Retry: sibling nodes under the same user node; lineage unique-active
  constraint (index backstop) + cooldown (Retry-After / exponential backoff,
  absolute time persisted) + UI double-submit guard; old attempts hidden by
  default (Attempt N badge, expandable); by default the original snapshot
  (model/params/context) is reused.
- Egress gate dialog: a single dialog for Blocked/Unknown; Re-check (no
  auto-send) / cancel / for mixed targets "Remove Anthropic models and
  send"; Esc closes; the explanation covers all Anthropic models (not only
  Claude).
- Model picker: family grouping (group titles 10px muted, non-interactive) +
  search box + adaptive height (150–300px); message cards/sidebar use
  normalized names with the raw ID in the tooltip; selection/persistence/
  billing still use the original ID.
- Status view model: `status_of` built from the generations authoritative
  state (historical/restart/late-completing failures all display); broadcast
  lag triggers a throttled DB reconciliation.

### Verification

- `cargo fmt --check` ✅; `cargo clippy --all-targets -D warnings` ✅;
  `cargo test --all-targets`: 201 passed / 0 failed
  (lib 151 + egress_guard 9 + request_recovery 12 + p2 15 + the rest).
- Acceptance matrix (A/B/C/D) and evidence: see section 0.1.4 of
  docs/ACCEPTANCE.md.
- GUI (dist exe + isolated data + local mock): model-picker-groups /
  anthropic-egress-unknown-modal (zero generation POSTs) /
  request-recovery-success-card / request-recovery-failed-card.
- `dist/aicouncil.exe` 0.1.4 SHA256:
  DCD7CED1BDA70F30182D66ECBA84A8030727E00B7490CAE6F87EC303C22E4B67

### Known Limitations (scope of this round)

1. Egress consistency boundary: GeoIP sees the detection service's egress;
   domain-based splitting / dual-stack / proxies dynamically switching
   egress may still diverge from the upstream (stated explicitly in Section
   3 of the plan; no guarantee claimed). `AICOUNCIL_PROXY` can pin a single
   unified egress.
2. Real-world proxy/dual-stack behavior not verified online (A11 recorded as
   not run; only code paths and unit coverage); real 401/402/429 account
   semantics not verified with a real key.
3. D06/D07 keyboard navigation and the DPI matrix, and the minimal-window
   layout, not verified item by item via screenshots; the picker's keyboard
   behavior relies on Iced's self-drawn list (up/down keys/Enter/Esc) and
   has no explicit assertions.
4. The dialog takes the "input-area replacement" form (not an overlay
   modal); under mixed-DPI multi-monitor setups the screenshot script crops
   in physical pixels.
5. B14 broadcast-backlog reconciliation is a throttled full refresh; C08
   keepalive 10-minute no-progress cap has no timing test (constant is
   tunable).
