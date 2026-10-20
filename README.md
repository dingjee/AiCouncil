# AiCouncil

A minimal native Rust desktop AI chat client for OpenRouter, built with iced.
English UI, local SQLite persistence, no telemetry.

## Features

- Automatic OpenRouter model catalog with a unified model picker: single pick,
  or same-question multi-model parallel answers from one entry. Family-grouped
  list with search; a star marks the primary model.
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
- Automatic chat titles via DeepSeek (optional); manual renames always win.
- Soft Sage reading theme, draggable right pane, Markdown export, exact cost
  labels.

Details: [implementation status](docs/IMPLEMENTATION_STATUS.md) ·
[rendering notes](docs/RENDERING.md) ·
[acceptance matrix](docs/ACCEPTANCE.md).

## Run (Windows 10/11)

Requires Rust (toolchain pinned by rust-toolchain.toml).

```sh
cargo run --release
```

The data directory is created automatically next to the exe on first launch:

- `data\aicouncil.db` — chats, message tree, generation records, cost ledger
  (SQLite/WAL; old databases migrate automatically; a missing local database
  is migrated one-way from legacy `%APPDATA%\AiCouncil`)
- `data\config.json` — non-sensitive settings
- `data\credentials.dpapi` — API keys, DPAPI-encrypted for the current
  Windows user; never written to plain config, the database, or exports

## Offline development and checks

```sh
# local mock (catalog + SSE streaming + costs), no real spend
cargo run --example mock_openrouter -- 8901
# another terminal
AICOUNCIL_BASE_URL=http://127.0.0.1:8901/v1 cargo run

# rendering acceptance window (formulas / CJK-mixed / stress samples)
cargo run --example render_probe

# automated E2E against the mock (=multi adds parallel, =multi2 adds
# multi-branch follow-ups)
AICOUNCIL_E2E=multi cargo run

# routine checks
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

(On memory-constrained machines, add `-j 2` to cargo commands.)

## Known limitations

- tiny-skia software rendering (wgpu produces no frames in remote-desktop
  sessions; on GPU machines the wgpu feature can be re-enabled in Cargo.toml)
- in-message selection only; continuous drag-select across messages is not
  implemented
- macOS / Linux unverified; real OpenRouter / DeepSeek integration not yet
  executed — everything is verified against the local mock

See docs/ACCEPTANCE.md for the full verified / not-run matrix.

## License

MIT — see [LICENSE](LICENSE). Icons are from Lucide (see
assets/icons/LUCIDE_LICENSE.md).
