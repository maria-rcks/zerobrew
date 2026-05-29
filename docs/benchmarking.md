# Benchmarking

zerobrew has two benchmark layers:

- `just bench` compares end-to-end install behavior against Homebrew using Hyperfine. It depends on local Brew/zerobrew state and is best for release checks.
- `just bench-fns` runs deterministic function-level benchmarks across `zb_core`, `zb_io`, and `zb_cli`. It is the right starting point when looking for slow internal code paths to hand off for optimization.

## Function-Level Benchmarks

Run:

```sh
just bench-fns
```

The recipe runs the Criterion suite in `zb_bench/benches/workspace_hotspots.rs`, then reads the generated Criterion `**/new/estimates.json` files and prints the slowest and fastest benchmarked functions by mean time. The slowest list is intended to become the optimization backlog.

The current suite covers representative hot paths from every crate:

- `zb_core`: formula token parsing, bottle selection, dependency closure resolution, and build plan generation.
- `zb_io`: formula suggestion ranking and privileged path validation.
- `zb_cli`: common CLI parse paths.

All inputs are local and synthetic, so the suite should avoid network, Homebrew state, and filesystem layout dependencies.

## End-to-End Install Benchmarks

Run:

```sh
just bench --quick
```

The recipe uses Hyperfine to time three commands for every selected package:

1. `brew install <package>`
2. `zb install <package>` after `zb reset -y` for a cold zerobrew cache
3. `zb install <package>` after a priming install and uninstall for a warm zerobrew cache

Use `-c, --count N` to limit the selected package list, and `--runs N` to set the Hyperfine run count per command. For example, `just bench --quick -c 3 --runs 5` benchmarks the first three quick packages with five Hyperfine runs per command. `--dry-run` prints the selected packages, output settings, and run count without running Hyperfine.

The Homebrew benchmark uses Homebrew's normal download cache between Hyperfine runs. This reflects repeat install performance on a typical developer machine rather than a forced first-download benchmark.

The default output is zerobrew's summary table. `--format json`, `--format csv`, `--format html`, or `--output <file>` keep the existing report formats while Hyperfine provides the timing measurements.

## Adding A Hotspot

Add new cases to `zb_bench/benches/workspace_hotspots.rs` when a function becomes important enough to optimize. Prefer public APIs and deterministic inputs. For expensive functions, include both a small case and a larger case so regressions show up before users feel them.

When comparing optimization work:

```sh
cargo bench -p zb_bench --bench workspace_hotspots -- --save-baseline main
cargo bench -p zb_bench --bench workspace_hotspots -- --baseline main
cargo run --quiet -p zb_bench
```

Criterion keeps the detailed statistical reports in `target/criterion/` or `zb_bench/target/criterion/`, depending on the Cargo bench target directory layout.
