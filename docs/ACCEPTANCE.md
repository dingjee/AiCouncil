# Acceptance Matrix Conclusions (PLAN Section 6)

Date: 2026-10-18 · Environment: Windows 10 19045 x64 (Remote Desktop session) · Rust 1.99.1

| ID | Scenario | Result | Evidence |
| --- | --- | --- | --- |
| R1 | Four delimiter types/fractions/integrals/matrices/cases/aligned | ✅ Passed (automated + screenshots) | `tests/math_coverage.rs` all render as SVG; `docs/screenshots/` (full page + zoomed crops) |
| R2 | `$` inside code/currency/unclosed `$$`/unknown macros | ✅ Passed | `render::md` 27 parsing tests (currency/code protection/literal fallback); `\LaTeX` macro falls back to source text as expected |
| R3 | Long content scrolling | ✅ Passed (GUI) | render_probe stress sample (20,000 chars + 50 formulas) pages and scrolls normally; input area stays responsive |
| B1 | Q→A1/B1 defaults to A1 | ✅ Passed | primary is determined before sending (`Planner` tests + Submitted receipt sets main_leaf); not by return order |
| B2 | Two follow-up branches checked, each with its own history | ✅ Passed (E2E) | Two-stage autopilot: 4 POSTs; DB tree has one user node per branch with identical content but different IDs; no cross-branch mixing; no implicit four requests |
| B3 | Main request failure does not auto-reselect | ✅ Passed (code path) | main_leaf updates only when the main request Finishes; failure only marks status; incomplete conversations cannot be continued (ContinueFrom validation) |
| B4 | Interleaving/cancel/session switch | ✅ Passed | `p2_mock_stream`: cancelled branch keeps received text while the other completes; events routed by ID; DB is authoritative |
| S1 | Force kill and restart | ✅ Passed | `p1_lifecycle::restart_recovery_marks_interrupted`; app startup calls recover_interrupted |
| S2 | Delete non-empty group | ✅ Passed | `p1_lifecycle::delete_group_moves_conversations_to_ungrouped` + UI entry point |
| M1 | Catalog refresh keeps preferences | ✅ Passed | `catalog_refresh_keeps_preferences` + failure keeps the old cache (`CatalogCache` tests) |
| M2 | Auto does not invent/out-of-range override rejected | ✅ Passed | `catalog::compose_params` tests (optional params not sent when defaults are absent; out-of-range override reported as rejected) |
| C1 | Duplicate usage reports recorded only once | ✅ Passed | `usage_merge_dedup_and_no_regression` + `usage_upsert_idempotent_and_no_regression` |
| C2 | Zero/unknown/unqueryable/late | ✅ Passed | Known legitimately zero, Pending/Unavailable distinguished; late usage booked idempotently and updates tags |
| E1 | Export three scopes/failed nodes/no leakage | ✅ Passed (service layer) | `services::export` tests (Main/AllBranches/failure markers/atomic write/filename sanitization); UI Main+All buttons wired, Selected driven by the checked set |
| A1 | Text attachments into request/DB/export (0.1.2) | ✅ Passed | E2E `attachments_composed_into_request_prompt` (mock request body contains `===== 附件` and the content; user node stores original prompt + attachment fields); roundtrip/0002 migration tests; export appends `[附件 N]` |
| A2 | Long paste converted to attachment/limits (0.1.2) | ✅ Passed (pure functions + unit tests) | `paste_threshold_classification` (≥2500 chars, counted per character); try_push limits (256KB/10 items/1MB); chip saved/restored with the draft |
| A3 | Parallel temporary multi-select (0.1.2) | ✅ Passed (pure functions + unit tests) | `resolve_parallel_targets` (None follows the panel/Some keeps intersection order/main model placed first/empty invalid); UI screenshot ui012-parallel-picker-release.png |
| P1 | Three-platform credentials/IME | ⚠️ Partial | Windows Credential Manager + in-process fallback implemented and tested; IME on-device acceptance pending; macOS/Linux unverified |

## Check Commands (Actually Run)

```sh
cargo fmt --all -- --check     # Passed
cargo clippy --all-targets -- -D warnings  # Zero warnings
cargo test                     # 100 passed (85 lib + 3 math + 5 p1 + 7 p2, rerun on 0.1.2, 2026-10-18)
cargo build --release          # Passed; release binary verified with mock E2E (send→stream→persist; 0.1.2 includes attachment cases)
```

## 0.1.1 UI/Portability Supplementary Evidence (2026-10-18)

- Migration consistency: WAL records not yet checkpointed were migrated via the
  backup API and the source database was left unchanged
  (`migration_uses_consistent_wal_snapshot_and_preserves_source`).
- UI screenshots (final exe, isolated mock data): docs/screenshots/ui011-*.png
  (empty state / settings + usage expanded, parallel card, model panel, minimum
  window, icon comparison); details in docs/IMPLEMENTATION_STATUS.md "0.1.1 fixes".

## 0.1.2 Attachments/Parallel Multi-select Supplementary Evidence (2026-10-18)

- Attachment E2E: the mock captures the request body and asserts attachment
  composition; the 0002 migration test simulates an old 0.1.1 database
  (`migration_0002_upgrades_old_database`).
- UI screenshots (final exe, isolated data directory): docs/screenshots/ui012-*.png
  (attachment chip bar, parallel multi-select panel expanded).

## Unverified/TODO

- Integration testing against real OpenRouter (requires a user-provided key; per the
  execution environment, authorization is required before real billing occurs).
- Chinese IME candidate confirmation with Enter must not accidentally send
  (manual on-device acceptance).
- macOS / Linux build and run (no matching machines, unverified, no cross-platform
  delivery claim made).
- Code signing/notarization (no certificate, marked incomplete).

## 0.1.3 UI Foundation Mechanism Overhaul Acceptance (2026-10-19)

This round was implemented per `docs/UI_FOUNDATION_PLAN.md` (that plan was removed
after completion; process records are in this file and in
IMPLEMENTATION_STATUS/RENDERING).

### Automated Checks (Actually Run, Windows 10 19045 x64 · Rust 1.99.1)

```sh
cargo fmt --all -- --check        # ✅ Passed
cargo clippy --all-targets -- -D warnings  # ✅ Zero errors
cargo test                        # ✅ 146 passed / 0 failed
```

- 125 lib tests (new: strict-vs-compatible emphasis differential T09, table empty
  cell T11, copy projection T12, streaming prefix selection stability T14, chunk
  consistency T10 simplified, Soft Sage contrast baseline T17 pure computation,
  scroll state machine T02 hysteresis/programmatic scroll/selection suspension,
  selection resolver T04, default parallel pairs, collapse heuristics, title
  CAS/ledger T06/T07, 0003 migration T08, usage ranking T05, i18n, word
  boundaries/graphemes).
- Integration tests: p1_lifecycle (5) / p2_mock_stream (7) / math_coverage (3) /
  p_title_metadata (6) all passed; no regression in existing behavior.

### Acceptance Matrix Mapping (plan Section 10 numbering)

| ID | Status | Notes |
| --- | --- | --- |
| T02 follow state | ✅ Automated | `app::scroll` hysteresis transitions/programmatic scroll suppression/one action per round/Selecting suspension (unit tests) |
| T04 selection resolver | ✅ Automated | `resolve_selection` single/parallel/main item promotion/empty set invalid; Send/estimate/plan share the same NormalizedSelection |
| T05 ranking | ✅ Automated | `model_usage_rank` stable sort by Started count + most recent use + model ID; not dequeued means not counted; title requests tracked in a separate ledger |
| T06/T07 title state machine | ✅ Automated | fallback→in_flight→ready/failed; CAS discards late results; manual not overwritten; restart recovers needs_generation; 0003 migration protects existing titles |
| T08 migration | ✅ Automated | `migration_0003_protects_existing_titles` (upgrade from an old 0.1.2 database; single-migration transaction rollback semantics implemented in migrate()) |
| T09 Markdown | ✅ Automated | Real-world positive samples (Chinese + quoted bold) + 7 negative-case differential groups (multiplication/single asterisk/underscore/hashtag/unclosed etc., Strict==LlmCompatible) |
| T10 chunk consistency | ✅ Simplified | Concatenation and re-parse at fixed split points == full parse; random-boundary streaming fuzz not done (listed as a limitation) |
| T11 tables | ✅ Automated | Empty cell preservation/missing column padding/formula headers; row height/borders rendered as layout properties (native checks not done) |
| T12 copy projection | ✅ Automated | Bold/links/formulas in all four delimiters/code line breaks/table tabs/list markers/quote prefixes/blank lines across blocks/partial cell |
| T13 Unicode hit testing | ✅ Partial | Grapheme word boundaries/grapheme substrings (CJK + emoji) pure-function tests; Paragraph hit_test depends on the iced implementation; native drag-select unverified |
| T14 streaming selection | ✅ Automated | Prefix selection projection stable (appends do not rewrite); reinterpretation cases preserve semantic position via re-render |
| T15 localization | ✅ Automated | Full en resource table; missing key falls back to the key itself; interpolation/plurals; unsupported locale falls back to en (no switching UI at the entry point; per plan, en only this round) |
| T16 configured width | ✅ Partial | Invalid/NaN/out-of-range falls back to 300 and clamps (240–480, window width constraint); persisted to disk only when the drag ends (spawn_blocking); NaN filtered with is_finite |
| T17 theme | ✅ Pure computation | Soft Sage token contrast unit tests (body ≥4.5:1, borders ≥3:1, unrounded formulas); native monitor verification not done |
| V01–V11 native UI | ⚠️ Unverified | This round ran no native window/screenshot/screen-recording acceptance (live reproduction of doubled typed input, streaming scroll appearance, partial-copy clipboard read-back, DPI matrix, etc.); **checked-off plan items cannot serve as evidence; the table above covers only the automated portion** |

### Evidence and Artifacts

- Final deliverable: `dist/aicouncil.exe` (0.1.3, SHA256 below), hash-identical to
  `target/release/aicouncil.exe`:
  `E3BFEBAC7AE7A02BA6BE4FC00A6CE1B27C4DA40C1C7E0F067EF642728C3A28A4`.
- Startup smoke test: boots and stays alive with an isolated data directory; Soft Sage
  colors/English UI/empty state render correctly
  (docs/screenshots/ui013-soft-sage-release.png; sidebar drawing is affected by DPI
  virtualization in the current RDP session, see IMPLEMENTATION_STATUS "Issues found
  and fixed during smoke testing" — layout tree dumps prove node positions are
  correct; pixels from this session are not accepted as layout evidence and require
  re-verification in a real monitor session).
- Source-level evidence: per-module `#[cfg(test)]` (render pipeline refactored into
  pure functions, rerunnable without a GUI); `examples/render_probe.rs` adapted to the
  new rendering interface.

### Unverified/TODO (Carried Over from Previous Round + New This Round)

- All previously unverified items remain in effect (real OpenRouter integration
  testing, IME on-device, macOS/Linux, code signing).
- New this round: V01–V11 native UI acceptance (including rechecking the sidebar
  drawing misalignment in this RDP session), T10 random-chunk fuzz, T13 native
  drag-select clipboard read-back, title integration testing against real DeepSeek
  (`.env` reading implemented; no real requests sent to verify response sanitization).

## 0.1.3 Release Record

```sh
cargo fmt --all -- --check                 # ✅ Passed
cargo clippy --all-targets -- -D warnings  # ✅ Zero warnings
cargo test                                 # ✅ 146 passed / 0 failed
cargo build --release                      # ✅ Done
# dist copy done, both hashes match:
# E3BFEBAC7AE7A02BA6BE4FC00A6CE1B27C4DA40C1C7E0F067EF642728C3A28A4
```

`docs/UI_FOUNDATION_PLAN.md` was removed after implementation per user instruction;
its research conclusions, implementation mapping, and unverified list are preserved in
this file, docs/RENDERING.md (2026-10-19 section), and
docs/IMPLEMENTATION_STATUS.md (0.1.3 section). dist/data/ is the user's real runtime
data (database + credentials) and was not touched.

---

## 0.1.4: Egress Gate / Request Recovery / Picker Acceptance Matrix (2026-10-19)

Execution environment: Windows, local countable mock (tests/common +
examples/mock_openrouter), isolated `AICOUNCIL_DATA_DIR`, no public internet, no real
keys, no real billed requests.
✅ = automated tests passed; 🖼️ = dist exe GUI screenshot evidence; ❑ = not run (see
limitations).

### A. Geo and Send Gate

| ID | Result | Evidence |
| --- | --- | --- |
| A01 | ✅ | tests/egress_guard.rs `a01_…`: CN/HK × Claude/non-Claude fixtures → POST=0, batch/gen/assistant delta=0, AdmissionError::EgressBlocked |
| A02 | ✅ | `a02_…`: US/JP/SG/MO/TW allowed through, body contains the original model ID (ban not extended to MO/TW) |
| A03 | ✅ | `a03_a14_…` (Unknown blocked) + egress.rs unit tests (payload validation: missing code/ZZ/reserved IP/200 error) |
| A04 | ✅ (logic) | egress.rs `run_detection` primary source failure → one fallback attempt; valid CN/HK not overwritten (source path + unit-test classification) |
| A05 | ✅ | `a05_…`: non-Anthropic target + CN guard → guard called zero times, sends normally |
| A06 | ✅ | `a06_…`: mixed 3 targets rejects the whole batch (0 POSTs/0 persisted); the new plan leaves only non-Anthropic sendable |
| A07 | ✅ | `a07_…`: 3 Anthropic-family targets in the same batch → 1 detection, 3 POSTs |
| A08 | ✅ | `a08_…` (dispatch level: concurrency 1 + slow stream queued >TTL → egress flips to CN → Failed/NotDispatched, usage unavailable, already-sent request stays Completed); `a08_stale_permit_…` (admission-level expiry/revision) |
| A09 | ✅ | `a09_…`: service retry + Blocked → AdmissionError, 0 new generations |
| A10 | ✅ (service) | submit freezes the snapshot + intent validation (on_egress_checked); the App-layer receipt clears only the submitted revision |
| A11 | ❑ | Real proxy not verified online; code path: unified NetworkConfig, explicit error on proxy failure (unit test `clients_build_with_and_without_proxy`) |
| A12 | ✅ | `a08_stale_permit_…`: fresh detection for every action (check count increments) |
| A13 | ✅ | `a01_…` includes `anthropic/test-non-claude-family` (blocked even without the literal "Claude") |
| A14 | ✅ | `a03_a14_…`: mock/claude-lookalike not blocked; claude-* aliases blocked |

### B. Lifecycle, Context, and Retry

| ID | Result | Evidence |
| --- | --- | --- |
| B01 | ✅ | request_recovery.rs `b01_…`: new question succeeds after the failure, old card kept, one user node each, POST=2 |
| B02 | ✅ | `b02_…`: recover_interrupted → Interrupted (state from DB, zero new generations, explicit retry possible) |
| B03 | ✅ | `b03_…`: captured body contains the U1→U2 order, no error text/empty assistant |
| B04 | ✅ | `b04_…`: partial text kept and carried into the next round |
| B05 | ✅ (policy) | plan.rs `empty_assistant_placeholder_filtered_…` + App `continuation_ready` (blocks only in the active state; terminal states pass) |
| B06 | ✅ (equivalent) | main_leaf CAS (b13) + the Continue button still requires Completed; "continue from here" happens only on explicit user click |
| B07 | ✅ | `b07_…`: 10 concurrent retries → 1 success, 9 Duplicate, 1 sibling, POST delta=1, user not duplicated |
| B08 | ✅ | `b08_…`: two lineages on the same model each retry successfully (4 generations total) |
| B09 | ✅ | `b09_b10_…`: 403→Forbidden (not collapsed into a key error); classification mapping in providers/mod.rs `failure_kind` |
| B10 | ✅ | `b09_b10_…`: Retry-After 120s → cooldown persisted; retry=Cooldown and zero POSTs before expiry |
| B11 | ✅ | `b11_…`: foreign key failure → Storage error, 0 POSTs, 0 fake messages (keeping the draft is App-layer behavior) |
| B12 | ✅ (service) | Receipts merged according to actual DB state (on_submitted reads generations; Queued is no longer resurrected) |
| B13 | ✅ | `b13_…`: late completion does not snatch back a leaf the user moved away from (main_leaf CAS) |
| B14 | ✅ (logic) | subscription Lagged → throttled Reconcile → DB reconciliation + cleanup of the terminal-state UI mapping |
| B15 | ✅ | `b15_…` + `b07_…`: cancel right after submit → Cancelled (token registered first) |
| B16 | ✅ | `b16_…`: retry reuses the snapshot (model/SYS/params/prompt; body assertions) |
| B17 | ✅ | `b17_…`: lineage=2, same user node, usage tracked per request; UI aggregation = Attempt N badge |
| B18 | ✅ (partial) | DB persist failure → GenEvent::PersistError visible toast (B18 real fault injection not done; error path reachable via unit tests) |

### C. HTTP / SSE / usage

| ID | Result | Evidence |
| --- | --- | --- |
| C01 | ✅ | p2 `c01_…`: top-level error+DONE / finish_reason=error → Failed |
| C02 | ✅ | `c02_…`: partial+EOF → Failed/StreamAborted, text preserved, no resend |
| C03 | ✅ | `c03_…`: only DONE → EmptyResponse; bad JSON → ParseFailure |
| C04 | ✅ | `c04_…`: text+usage+finish all three in the same frame |
| C05 | ✅ | `c05_…` + original `duplicate_terminal_events_…`: single terminal state, single cost entry |
| C06 | ✅ | `c06_…`: EOF after finish (no DONE) still Completed; cost not written as 0 |
| C07 | ✅ | `c07_…`: length with content = Completed+Truncated; content_filter semantics preserved |
| C08 | ✅ (constants) | read_timeout 120s / NO_PROGRESS 600s centralized in network.rs and openrouter.rs (timing fault injection not done) |
| C09 | ✅ | Original p2 UTF-8 across chunks/SSE comments/late usage cases keep passing |
| C10 | ✅ | `c10_…`: frames after the error do not overwrite the first terminal state |

### D. Picker and Visuals

| ID | Result | Evidence |
| --- | --- | --- |
| D01 | 🖼️+✅ | docs/screenshots/model-picker-groups.png (group titles small, muted); model_picker.rs unit tests |
| D02 | ✅ | model_identity unit tests (variant preserved/disambiguation/tooltip provided by the UI); the sent ID remains exactly unchanged (C-series asserts the original ID) |
| D03 | ✅ | model_picker.rs `search_matches_…` (family/short name/original name/raw ID; empty-result explanation in UI) |
| D04 | ✅ | `many_single_family_groups_…` (30 groups on a single scrollable page; height 150–300 adaptive) |
| D05 | ✅ | The original 3 unit tests of app/mod.rs resolve_selection keep passing |
| D06 | ❑ | Keyboard navigation of the Iced custom-drawn list not verified key by key (Enter/Esc go through the global KeyPressed) |
| D07 | 🖼️ (partial) | Screenshots at both 1456×898 and 1747×1076 window sizes look normal; the 100%/150% DPI matrix not covered item by item |
| D08 | 🖼️ | request-recovery-failed-card.png (short reason "Provider server error" + details tooltip + retry + input area usable) |

### Automation and Release Commands (0.1.4)

```text
cargo fmt --all -- --check                 # ✅ Passed
cargo clippy --all-targets -- -D warnings  # ✅ Zero warnings
cargo test --all-targets                   # ✅ 201 passed / 0 failed
cargo build --release                      # ✅ 0.1.4
Copy-Item target\release\aicouncil.exe dist\aicouncil.exe
Get-FileHash dist\aicouncil.exe -SHA256
# DCD7CED1BDA70F30182D66ECBA84A8030727E00B7490CAE6F87EC303C22E4B67
```

GUI verification (dist exe + isolated data + mock): see IMPLEMENTATION_STATUS.md
section 0.1.4. Not-run items (A11 real proxy, D06 keyboard, D07 DPI matrix, B18 fault
injection) are listed as limitations; "all passed" is not claimed.

## 0.1.5: Markdown Line Spacing and Table Regression (2026-10-19)

- `cargo test --release --lib --test render_layout --test math_coverage`: 158 passed, 0 failed (154 unit tests + 3 formula tests + 1 whole-document layout regression).
- Whole-document layout tests cover 12/15/22px body text × 360/760/1100px message widths, asserting text never overflows its paragraph, table backgrounds fill the entire row, column boundaries are consistent, empty cells keep a minimum line height, and paragraphs after a divider do not overlap the preceding block.
- Paragraph unit tests cover 90/180/360px widths, checking that the actually drawn line coordinates match selection hit testing; also verifies English word wrapping, centered/right alignment, and heading text/formula font sizes.
- `rustfmt --edition 2021 --check src/render/flow.rs src/render/view.rs tests/render_layout.rs examples/render_probe.rs` and `git diff --check` pass.
- Root cause and reproduction commands are in section 0.1.5 of `docs/RENDERING.md`.
- Screenshots: `docs/screenshots/markdown-0.1.5/`, 9 in total, generated directly by the same tiny-skia renderer and full `view_message` layout as the app. Visual inspection of regular size/medium width (15px/760px), large text on a narrow screen (22px/360px), and large text on a wide screen (22px/1100px) shows no paragraph overlap, gray broken blocks, or dividers cutting through text. The window capture tool timed out twice; this round's visual evidence is renderer offscreen output and does not count as real window screenshots.
- `cargo build --release --bin aicouncil --example render_probe` passes; delivered `dist/aicouncil.exe` (0.1.5); SHA256: `D638CBF8DAF110D3898B3199AFAA0542FC36EE27C4940C39CCCD94A5EBA5D213`.
- `target/` was not automatically cleaned up this round and is left for manual deletion (build artifact and delivered EXE hashes verified identical; the acceptance window process has been closed).
