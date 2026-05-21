use std::cmp::Ordering;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Estimates {
    mean: Estimate,
}

#[derive(Debug, Deserialize)]
struct Estimate {
    point_estimate: f64,
}

#[derive(Debug)]
struct Measurement {
    name: String,
    mean_ns: f64,
}

fn main() {
    let root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_criterion_dir);

    let mut measurements = Vec::new();
    collect_measurements(&root, &root, &mut measurements);

    if measurements.is_empty() {
        eprintln!(
            "No Criterion estimates found under {}. Run `just bench-fns` first.",
            root.display()
        );
        std::process::exit(1);
    }

    measurements.sort_by(|a, b| {
        b.mean_ns
            .partial_cmp(&a.mean_ns)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });

    println!("Slowest benchmarked functions");
    print_measurements(measurements.iter().take(10));

    println!();
    println!("Fastest benchmarked functions");
    print_measurements(measurements.iter().rev().take(10));
}

fn default_criterion_dir() -> PathBuf {
    let workspace_target = PathBuf::from("target/criterion");
    if workspace_target.exists() {
        workspace_target
    } else {
        PathBuf::from("zb_bench/target/criterion")
    }
}

fn collect_measurements(root: &Path, path: &Path, measurements: &mut Vec<Measurement>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_measurements(root, &path, measurements);
            continue;
        }

        if path.file_name().and_then(|name| name.to_str()) != Some("estimates.json") {
            continue;
        }
        if !path
            .components()
            .any(|component| component.as_os_str() == "new")
        {
            continue;
        }

        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(estimates) = serde_json::from_str::<Estimates>(&contents) else {
            continue;
        };

        if let Some(name) = benchmark_name(root, &path) {
            measurements.push(Measurement {
                name,
                mean_ns: estimates.mean.point_estimate,
            });
        }
    }
}

fn benchmark_name(root: &Path, path: &Path) -> Option<String> {
    let new_dir = path.parent()?;
    let benchmark_dir = new_dir.parent()?;
    let relative = benchmark_dir.strip_prefix(root).ok()?;
    let name = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    (!name.is_empty()).then_some(name)
}

fn print_measurements<'a>(measurements: impl Iterator<Item = &'a Measurement>) {
    for (idx, measurement) in measurements.enumerate() {
        println!(
            "{:>2}. {:<72} {:>12}",
            idx + 1,
            measurement.name,
            format_duration(measurement.mean_ns)
        );
    }
}

fn format_duration(ns: f64) -> String {
    if ns >= 1_000_000_000.0 {
        format!("{:.3} s", ns / 1_000_000_000.0)
    } else if ns >= 1_000_000.0 {
        format!("{:.3} ms", ns / 1_000_000.0)
    } else if ns >= 1_000.0 {
        format!("{:.3} us", ns / 1_000.0)
    } else {
        format!("{ns:.3} ns")
    }
}
