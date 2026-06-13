use std::cmp::Ordering;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use criterion_table::{build_tables, formatter::GFMFormatter};
use serde::{Deserialize, Serialize};

const TABLES_CONFIG: &str = "tables.toml";
const DEFAULT_TABLE_COLUMN: &str = "input";

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

#[derive(Debug, Deserialize)]
struct BenchmarkMetadata {
    full_id: String,
}

#[derive(Debug, Serialize)]
struct CriterionJsonLine {
    reason: &'static str,
    id: String,
    report_directory: String,
    iteration_count: Vec<u64>,
    measured_values: Vec<f64>,
    unit: String,
    throughput: Vec<Throughput>,
    typical: ConfidenceInterval,
    mean: ConfidenceInterval,
    median: ConfidenceInterval,
    median_abs_dev: ConfidenceInterval,
    slope: Option<ConfidenceInterval>,
    change: Option<ChangeDetails>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ConfidenceInterval {
    estimate: f64,
    lower_bound: f64,
    upper_bound: f64,
    unit: String,
}

#[derive(Debug, Serialize)]
struct Throughput {
    per_iteration: u64,
    unit: String,
}

#[derive(Debug, Serialize)]
struct ChangeDetails {
    mean: ConfidenceInterval,
    median: ConfidenceInterval,
    change: ChangeType,
}

#[derive(Debug, Serialize)]
enum ChangeType {
    NoChange,
}

#[derive(Debug, Deserialize)]
struct CriterionEstimates {
    mean: CriterionEstimate,
    median: CriterionEstimate,
    median_abs_dev: CriterionEstimate,
}

#[derive(Debug, Deserialize)]
struct CriterionEstimate {
    confidence_interval: CriterionConfidenceInterval,
    point_estimate: f64,
}

#[derive(Debug, Deserialize)]
struct CriterionConfidenceInterval {
    lower_bound: f64,
    upper_bound: f64,
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--table") {
        run_table_report(&args);
    } else {
        run_rank_report(&args);
    }
}

fn run_rank_report(args: &[String]) {
    let root = args
        .first()
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

fn run_table_report(args: &[String]) {
    let root = args
        .iter()
        .find(|arg| arg.as_str() != "--table")
        .map(PathBuf::from)
        .unwrap_or_else(default_criterion_dir);
    let json_lines = criterion_json_lines(root);
    if json_lines.is_empty() {
        eprintln!("No Criterion estimates found. Run `just bench-fns` first.");
        std::process::exit(1);
    }

    let report =
        build_tables(json_lines.as_bytes(), GFMFormatter, TABLES_CONFIG).unwrap_or_else(|err| {
            eprintln!("Failed to build Criterion table report: {err}");
            std::process::exit(1);
        });
    print!("{report}");
}

fn default_criterion_dir() -> PathBuf {
    let workspace_target = PathBuf::from("target/criterion");
    if workspace_target.exists() {
        workspace_target
    } else {
        PathBuf::from("zb_bench/target/criterion")
    }
}

fn criterion_json_lines(root: PathBuf) -> String {
    let mut rows = Vec::new();
    collect_criterion_json_lines(&root, &root, &mut rows);
    rows.sort_by(|a, b| a.id.cmp(&b.id));

    rows.into_iter()
        .map(|row| serde_json::to_string(&row).expect("Criterion table row should serialize"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_criterion_json_lines(root: &Path, path: &Path, rows: &mut Vec<CriterionJsonLine>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_criterion_json_lines(root, &path, rows);
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

        let Some(id) = benchmark_id(root, &path) else {
            continue;
        };
        let Ok(row) = criterion_json_line(&path, &id) else {
            continue;
        };
        rows.push(row);
    }
}

fn criterion_json_line(path: &Path, id: &str) -> serde_json::Result<CriterionJsonLine> {
    let contents = fs::read_to_string(path).unwrap_or_default();
    let estimates = serde_json::from_str::<CriterionEstimates>(&contents)?;
    let benchmark_dir = path.parent().and_then(Path::parent);
    let metadata_path = benchmark_dir
        .map(|path| path.join("benchmark.json"))
        .unwrap_or_default();
    let id = if let Ok(contents) = fs::read_to_string(metadata_path) {
        let metadata = serde_json::from_str::<BenchmarkMetadata>(&contents)?;
        table_id(&metadata.full_id)
    } else {
        table_id(id)
    };

    Ok(CriterionJsonLine {
        reason: "benchmark-complete",
        id,
        report_directory: benchmark_dir
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        iteration_count: Vec::new(),
        measured_values: Vec::new(),
        unit: "ns".to_string(),
        throughput: Vec::new(),
        typical: estimates.mean.confidence_interval("ns"),
        mean: estimates.mean.confidence_interval("ns"),
        median: estimates.median.confidence_interval("ns"),
        median_abs_dev: estimates.median_abs_dev.confidence_interval("ns"),
        slope: None,
        change: Some(ChangeDetails {
            mean: zero_percent(),
            median: zero_percent(),
            change: ChangeType::NoChange,
        }),
    })
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

fn benchmark_id(root: &Path, path: &Path) -> Option<String> {
    let new_dir = path.parent()?;
    let benchmark_dir = new_dir.parent()?;
    let relative = benchmark_dir.strip_prefix(root).ok()?;
    let id = relative
        .components()
        .filter_map(component_str)
        .collect::<Vec<_>>()
        .join("/");
    (!id.is_empty()).then_some(id)
}

fn table_id(id: &str) -> String {
    let mut parts = id.split('/');
    let Some(group) = parts.next() else {
        return id.to_string();
    };
    let Some(row) = parts.next() else {
        return id.to_string();
    };

    let column = encoded_column(parts);
    format!("{group}/{column}/{row}")
}

fn component_str(component: Component<'_>) -> Option<String> {
    let part = component.as_os_str().to_string_lossy();
    (part != "base" && part != "new").then(|| part.into_owned())
}

fn encoded_column<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let column = parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("__");
    if column.is_empty() {
        DEFAULT_TABLE_COLUMN.to_string()
    } else {
        column
    }
}

fn zero_percent() -> ConfidenceInterval {
    ConfidenceInterval {
        estimate: 0.0,
        lower_bound: 0.0,
        upper_bound: 0.0,
        unit: "%".to_string(),
    }
}

impl CriterionEstimate {
    fn confidence_interval(&self, unit: &str) -> ConfidenceInterval {
        ConfidenceInterval {
            estimate: self.point_estimate,
            lower_bound: self.confidence_interval.lower_bound,
            upper_bound: self.confidence_interval.upper_bound,
            unit: unit.to_string(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{DEFAULT_TABLE_COLUMN, encoded_column, table_id};

    #[test]
    fn table_id_returns_short_ids_unchanged() {
        assert_eq!(table_id("onlyone"), "onlyone");
        assert_eq!(table_id(""), "");
    }

    #[test]
    fn table_id_handles_empty_segments() {
        assert_eq!(table_id("group//row"), "group/row/");
        assert_eq!(
            table_id("group/col/"),
            format!("group/{DEFAULT_TABLE_COLUMN}/col")
        );
    }

    #[test]
    fn table_id_encodes_multi_segment_columns() {
        assert_eq!(encoded_column(["b", "c", "row"]), "b__c__row");
        assert_eq!(table_id("group/a/b/c/row"), "group/b__c__row/a");
    }
}
