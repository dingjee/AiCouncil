# AiCouncil

A minimal native AI chat client for OpenRouter. English UI, local SQLite
persistence, no telemetry. Version 0.2.0 ships as a Tauri 2 desktop app for
macOS (Apple Silicon); the previous Windows/iced build is superseded on this
branch (historical sources remain recoverable via git).

## Features

- Automatic OpenRouter model catalog with a unified model picker: single pick,
  or same-question multi-model parallel answers from one entry. Family-grouped
  list with search; a star marks the primary model.
- Parallel answers are one group: a tab strip over a fixed-height pane, so
  every answer stays visible however long it is, switching tabs never moves
  the page, and "Split" shows two answers side by side for comparison.
- Multi-branch follow-ups: branch from any completed answer, each branch keeps
  its own history; "continue from here" switches the main branch.
- Markdown + LaTeX rendering with in-message selection and copy; formulas copy
  as source with their original delimiters; body font size adjustable 12–22px.
- Text attachments (single file ≤256KB, ≤10 files / ≤1MB total) with automatic
  conversion of long pastes (≥2500 characters) into attachments.
- Anthropic-model egress gate: mainland China / Hong Kong / unknown network
  egress is blocked with an explicit dialog before any Anthropic model is
  called; non-Anthropic models are unaffected.
- Failure recovery: failed rounds don't lock the conversation; eligibility-
  gated retry with cooldowns; attempts aggregate per card.
- Composer drafts persist per chat and survive closing the app: non-blank input
  is restored exactly (whitespace and line breaks included), every chat keeps
  its own text and attachments, and "New chat" returns to the same unsent
  slot instead of creating empty shells.
- Automatic chat titles via DeepSeek (optional); manual renames always win.
- Soft Sage reading theme, draggable right pane, Markdown export, exact cost
  labels.

## Run on macOS (Apple Silicon)

Prerequisites: Xcode Command Line Tools, Rust stable (via
[rustup](https://rustup.rs); the toolchain pin lives in `rust-toolchain.toml`),
and [Bun](https://bun.sh) for the frontend build.

```sh
(cd frontend && bun install --frozen-lockfile && bun run build)
cargo build --release
```

Or use the repeatable scripts:

```sh
scripts/macos_check.sh              # fmt + clippy + tests (isolated data dir)
scripts/macos_build.sh              # cargo build --release, exe staged in dist/
scripts/macos_release_check.sh      # verify dist artifacts (arch/signature/deps)
```

## Data location (macOS)

- Bundled app: `~/Library/Application Support/AiCouncil/`
  - `aicouncil.db` — chats, message tree, generation records, cost ledger
    (SQLite/WAL; schema migrations apply automatically)
  - `config.json` — non-sensitive settings
  - `.env` — your API keys (see below; maintained by you, read-only to the app)
- First launch of the bundled app imports a legacy PC database found next to
  the `.app` bundle (`<bundle dir>/../data/aicouncil.db`, e.g. `dist/data`)
  via a WAL-safe SQLite snapshot: never overwrites an existing database,
  idempotent on later launches, source file left byte-identical.
- Tests and isolated runs: set `AICOUNCIL_DATA_DIR=/some/tmp/dir` — the app
  then uses exactly that directory and performs no legacy import.

## API keys (.env is the single source)

Put your keys in the data-directory `.env` (copy `.env.example` there):

```
OPENROUTER_API_KEY=...
DEEPSEEK_API_KEY=...
```

The app reads only these named variables, re-reads the file on every lookup,
and never writes, logs, or exports them. After placing or changing `.env`,
call the in-app reload (or restart): the OpenRouter and DeepSeek title clients
are rebuilt from the file. There is no Keychain or DPAPI storage in 0.2.0.

## Windows → macOS migration notes

- Chats/settings: the bundled app auto-imports the legacy PC `data/` directory
  that sits next to it (non-overwriting; see above). A byte-identical labeled
  backup of the original PC data is kept in `dist/pc-data-backup/`.
- API keys: the existing private `.env` (already containing both named keys)
  is copied to the runtime data directory; the app's reload then rebuilds both
  provider clients from it. The Windows `credentials.dpapi` ciphertext cannot
  be decrypted on macOS and is NOT imported — it stays only in the archived
  backup (`dist/pc-data-backup/`) for reference.

## Verification

- `docs/MACOS_ACCEPTANCE.md` — macOS migration/acceptance report with actual
  commands and results.
- `scripts/macos_migrate_selfcheck.sh` — verifies the real PC backup via
  hashes and a temp-dir copy (counts, integrity, idempotent import) without
  touching the original.

## Known limitations

- macOS-only on this branch; no Windows/Linux build support.
- Native UI behaviors (IME composition, window management) are covered by the
  frontend scope; browser-level checks supplement but do not replace native
  verification — see the acceptance report for exactly what was run.
- Ad-hoc local codesigning only; no Apple notarization is claimed.

## License

MIT — see [LICENSE](LICENSE). Icons are from Lucide (see
assets/icons/LUCIDE_LICENSE.md).
