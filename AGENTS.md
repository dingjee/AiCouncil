# AGENTS.md

- Every task end: delete temp/intermediate files; keep only source, docs, assets, and `dist/`.
- Every version change: `cargo build --release`, copy exe to `dist/`, then delete `target/` (it regrows to GBs every build).
