zerobrew is a Homebrew-compatible package manager written in Rust, with an architecture inspired by `uv` and a constant focus on being marginally faster than regular Homebrew. The project is organized as a Cargo workspace with `zb_core` for core logic, `zb_io` for I/O and installation work, and `zb_cli` for the command-line interface.

Always make code changes atomically. When building a larger feature, split the work into smaller commits so each commit has a clear purpose.

Use Conventional Commit-style messages scoped to the crate or area changed, for example `fix(zb_io): handle missing bottle metadata`. Prefer `fix`, `feat`, `chore`, `tests`, `ci`, `refactor`, `perf`, and `build`.

Add or update tests for behavior changes. Tests should be deterministic and avoid external network access.

Before committing locally, run `just fmt`, `just lint`, and `just test`. Fix the code or tests until all three pass.
