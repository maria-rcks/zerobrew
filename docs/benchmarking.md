# Benchmarking

zerobrew has two benchmark layers:

- `just bench` compares end-to-end install behavior against Homebrew. It depends on local Brew/zerobrew state and is best for release checks.
- `just bench-fns` runs deterministic function-level benchmarks across `zb_core`, `zb_io`, and `zb_cli`. It is the right starting point when looking for slow internal code paths to hand off for optimization.

## Function-Level Benchmarks

Run:

```sh
just bench-fns
```

The recipe runs the Criterion suite in `zb_bench/benches/workspace_hotspots.rs`, then reads `zb_bench/target/criterion/**/new/estimates.json` and prints the slowest and fastest benchmarked functions by mean time. The slowest list is intended to become the optimization backlog.

The current suite covers representative hot paths from every crate:

- `zb_core`: formula token parsing, bottle selection, dependency closure resolution, and build plan generation.
- `zb_io`: formula suggestion ranking and privileged path validation.
- `zb_cli`: common CLI parse paths.

All inputs are local and synthetic, so the suite should avoid network, Homebrew state, and filesystem layout dependencies.

## Adding A Hotspot

Add new cases to `zb_bench/benches/workspace_hotspots.rs` when a function becomes important enough to optimize. Prefer public APIs and deterministic inputs. For expensive functions, include both a small case and a larger case so regressions show up before users feel them.

When comparing optimization work:

```sh
cargo bench -p zb_bench --bench workspace_hotspots -- --save-baseline main
cargo bench -p zb_bench --bench workspace_hotspots -- --baseline main
cargo run --quiet -p zb_bench -- zb_bench/target/criterion
```

Criterion keeps the detailed statistical reports in `zb_bench/target/criterion/`.
