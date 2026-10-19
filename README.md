# AiCouncil

Minimal native Rust desktop AI chat: automatic OpenRouter model catalog,
unified model picker (single pick, or same-question multi-model parallel
answers from the same entry), multi-branch follow-ups from selected answers,
text attachments with long-paste auto-conversion, LaTeX formula rendering,
**in-message selection with formula-source copy**, Soft Sage reading theme,
adjustable body font size and right-pane width, automatic chat titles
(DeepSeek, optional), English UI, local SQLite persistence, Markdown export
and cost labels.

Current status: [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md);
rendering conclusions and known gaps: [docs/RENDERING.md](docs/RENDERING.md);
acceptance matrix and unverified items: [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md).

## A quick look

Unified model picker (family groups + Parallel multi-select, star sets the
primary model):

![Model picker](docs/screenshots/model-picker-groups.png)

Markdown/formula rendering (12/15/22px × three widths regression sample, see
docs/screenshots/markdown-0.1.5/ for the full set):

![Rendering regression](docs/screenshots/markdown-0.1.5/markdown-15px-760w.png)

## Run (Windows)

```sh
cargo run --release
```

Requires Rust 1.88+ (toolchain pinned to 1.99.1 by rust-toolchain.toml);
Windows 10/11. The data directory and database are created automatically on
first launch.

## Data location

- Data directory: `<folder containing aicouncil.exe>\data\`
  - `aicouncil.db` — chats, message tree, generation records, cost ledger,
    title metadata and the auxiliary request ledger (SQLite/WAL; old
    databases migrate automatically; existing non-empty titles are treated
    as manual and protected)
  - `config.json` — non-sensitive settings (body font size etc.; window:
    sidebar width)
- On first run, if the local database is missing, it is migrated from the
  legacy `%APPDATA%\AiCouncil` via SQLite online backup; legacy files are
  kept, and an existing local database is never overwritten.
- API keys: on Windows the key is encrypted with the current user's DPAPI and
  stored in `data\credentials.dpapi`, and imported from the legacy credential
  manager if present. The file cannot be decrypted by other Windows users. On
  other platforms keys are held for the current run only. **Keys are never
  written to plain config, the database, or exports.**
- The Windows data directory should be in a location writable by the current
  user. Copying the `data` folder to another machine migrates chat history,
  but encrypted credentials remain bound to the original Windows user.

## In-message selection and copy (0.1.3)

- Mouse drag selects **within a single message** across paragraphs, headings,
  lists, code blocks and tables; double-click selects a word, triple-click a
  block. Auto-follow scrolling pauses during selection; Esc or click clears.
- `Ctrl+C` / Copy yields a **plain-text projection** (not the Markdown
  source): bold/links contribute their decoded text, inline code its content,
  and **formulas return the source with original delimiters** (`$x^2$` copies
  verbatim; `\(x^2\)` stays in that form). Paragraphs are separated by blank
  lines, table cells by tabs, code keeps line breaks and indentation, list
  items keep their markers.
- The message card's "Copy" button (tooltip: Copy source) still copies the
  full Markdown source — clearly distinct from partial copy. `Ctrl+C` inside
  the input box is handled by the editor and is not intercepted.

## Unified model picker (0.1.4 family groups)

- The single picker lives at the bottom-left of the input area, grouped by
  **model family** (e.g. the Claude group shows "Opus 5.5", "Fable 5.1";
  same for GPT/Gemini/Kimi and standalone models). Group headers are small
  muted text and not clickable, clearly distinct from options;
  `:free`/`:thinking`/date variant info is preserved.
- Built-in search matches family, short name, full name, catalog display name
  and raw ID (case-insensitive); list height adapts to the window; closing
  the picker clears the search.
- In single-model mode a click selects. With "Parallel" enabled the same list
  turns into checkboxes, and a star marks the primary model (owner of the
  main branch). "Current + most-used other" are checked by default.
- Display and selection are layered: the UI shows normalized names (raw ID in
  the tooltip), while HTTP requests, preferences, billing, retries and export
  audits keep using the original model ID.
- While a multi-branch follow-up selection is active, Parallel is disabled
  with a hint (no implicit branch × model cartesian product).

## Anthropic model egress gate (0.1.4)

- Before sending or retrying any **Anthropic model** (all `anthropic/*`
  families and `claude-*` aliases, not just Claude-named models), the app
  checks the current public network egress: **mainland China (CN) / Hong Kong
  (HK) are blocked**, explicitly identified other countries/regions are
  allowed, and **Unknown is also blocked**. Non-Anthropic models skip this
  check.
- When blocked, a single dialog shows the detection observed this time
  (IP/country/source/time) with "Re-check" (re-detect only, never auto-send)
  or "Cancel"; if this round also targets non-Anthropic models, a third
  option "remove Anthropic models and send" appears. Edited text, attachments
  and model checks are all preserved; no request is sent and no record is
  created.
- If queueing exceeds the permit validity (3 seconds) or the network
  configuration changes, the check reruns before actually sending; requests
  denied on re-check are marked failed and their cost never enters
  "in-flight".
- Detection services: ip.sb (primary) → ipapi.co (fallback), total budget 8
  seconds; results live in short-term memory only, the public IP is never
  persisted. Set `AICOUNCIL_PROXY=http://host:port` to pin the same egress
  for detection and generation (a proxy configuration failure errors out
  explicitly, never silently falling back to direct connection).
- Boundary: GeoIP reports the egress as seen by the detection service;
  domain-based routing, dual stack, or proxies that rotate egress may still
  differ from what the upstream sees — this feature does not claim to
  guarantee the upstream-visible IP.

## Failure recovery and retry (0.1.4)

- Failures no longer lock the conversation: you can type a new question
  directly under a failed/cancelled/interrupted answer; the context
  automatically filters out empty failure placeholders (a "no answer" is
  never disguised as answered), and failed answers with partial output are
  kept and included in later context.
- Failed cards show a short reason (details in the tooltip, never sent to the
  model); SSE error frames, empty responses and truncations
  (length/content_filter) are labeled distinctly — failures are never
  disguised as empty successes.
- The retry button is eligibility-gated: cooldown shows a countdown (429
  honors Retry-After; network/5xx classes back off 2/4/8… up to 30 seconds,
  not reset by restart); credential/balance/parameter/model errors prompt for
  remediation first instead of encouraging futile clicking.
- Repeated retry clicks on one answer produce a single effective attempt (UI
  de-duplication plus a database unique constraint as backstop); retries
  reuse the original request snapshot (model/params/context/system prompt)
  by default, attempts aggregate as "Attempt N", older attempts hidden by
  default and expandable.
- Submit failures (disk/database errors) create no fake messages; drafts and
  attachments are recovered automatically.

## Automatic chat titles (0.1.3, optional)

- After the first successful send, a fallback title is generated immediately
  from the first question, and DeepSeek is asked in the background for a
  short title; on failure (no credentials/timeout/rate limit) the fallback
  stays and **one more try happens on the next question** — never multiple
  automatic retries within one round.
- Credentials: process env `DEEPSEEK_API_KEY` first; otherwise the same
  variable read from `.env` (development = project root, release = next to
  the exe; `AICOUNCIL_ENV_PATH` overrides the path; template in
  `.env.example`). **Only the named variable is read; the whole .env is never
  imported; keys never reach logs, the database, or the UI.**
- Manual renames (pencil icon next to the main-area title, or in the sidebar)
  always win; late automatic results are dropped by database version number
  and never overwrite a manual title.

## UI and settings (0.1.3)

- Soft Sage theme: warm gray paper + off-white body + low-saturation
  gray-green accents (whole app, including code highlight backgrounds and
  formula colors); Settings has a body-size slider (12–22px), applied live
  and persisted.
- Right-pane width is draggable (240–480, double-click resets to 300);
  written to disk only after the drag ends; collapse state survives restart.
- All UI strings are English (resources centralized in `src/app/i18n.rs`;
  user content, model IDs and raw provider errors are never translated).

## Text attachments and long paste (0.1.2)

- The input area "Attach" button picks multiple text/code files (single file
  ≤256KB, ≤10 files / ≤1MB total; over-limit files are skipped with a toast;
  non-UTF-8 files error and are skipped).
- Pasting ≥2500 characters converts the text into an attachment
  automatically (same threshold as Claude.ai) instead of blowing up the
  input box.
- Attachments are **plain-text concatenation**: on send they are joined into
  the prompt with `===== 附件 N: 名称 =====` separators, transparent to all
  models; persisted locally with the message and chat draft; Markdown export
  appends the full attachment text after the user message.

## Offline development and acceptance

```sh
# Local mock (catalog + SSE streaming + costs), no real spend
cargo run --example mock_openrouter -- 8901
# another terminal
AICOUNCIL_BASE_URL=http://127.0.0.1:8901/v1 cargo run

# Rendering acceptance window (formulas / CJK-mixed / stress samples)
cargo run --example render_probe

# Automated E2E (mock): =multi for dual-model parallel; =multi2 adds
# multi-branch follow-ups
AICOUNCIL_BASE_URL=http://127.0.0.1:8901/v1 AICOUNCIL_E2E=multi cargo run

# Screenshot/demo helper (isolated data dir, no real data migration):
# start panel + usage expansion
AICOUNCIL_DATA_DIR=/tmp/demo-data AICOUNCIL_DEMO_MODE=1 \
AICOUNCIL_DEMO_PANEL=settings AICOUNCIL_DEMO_USAGE=1 cargo run

# Screenshot helper: =chips preloads an attachment chip; =parallel expands
# the parallel multi-select panel
AICOUNCIL_DATA_DIR=/tmp/demo-data AICOUNCIL_DEMO_MODE=1 \
AICOUNCIL_DEMO_ATTACH=parallel cargo run
```

## Routine checks

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

(On memory-constrained Windows machines, add `-j 2` to cargo commands if
parallel rustc hits paging-file limits.)

## Known limitations

- The renderer uses tiny-skia software rendering (wgpu produces no frames in
  a local remote-desktop session; on GPU machines the wgpu feature can be
  re-enabled in Cargo.toml).
- In-message selection is currently **within one message**; continuous
  drag-select across messages is not implemented (see the new gap in
  docs/RENDERING.md).
- Real-DeepSeek integration for automatic titles has not been executed
  (implementation verified against the offline contract; response cleaning
  has unit tests, no real request).
- Real OpenRouter integration requires a user-provided key; all features are
  currently verified against the local mock.
- macOS / Linux unverified (no platform-specific code; credentials/dialogs
  use cross-platform libraries).
- Egress-gate consistency boundaries with real proxies/dual stack, the
  picker's per-key keyboard navigation matrix, and the 100%/150% DPI
  screenshot checklist were not run this round (see docs/ACCEPTANCE.md
  0.1.4 not-run list).

## License

MIT — see [LICENSE](LICENSE). Icons are from Lucide (see
assets/icons/LUCIDE_LICENSE.md).
