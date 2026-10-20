# AGENTS.md

- Every task end: delete temp/intermediate files; keep only source, docs, assets, and `dist/`.
- Screenshots (any verification/UI capture, e.g. `*.png`/`*.jpg` under a `screenshots/` directory) are never committed: they are gitignored; delete them at task end along with other temp files. Never `git add` a capture; if one slips into history, purge it from all commits (filter-branch/filter-repo) and force-push before handing over.
- Every version change: `cargo build --release`, copy exe to `dist/`, then delete `target/` (it regrows to GBs every build).
