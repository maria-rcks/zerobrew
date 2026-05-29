use std::collections::BTreeMap;
use std::hint::black_box;
use std::path::{Path, PathBuf};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use zb_cli::cli::Cli;
use zb_core::formula::{
    Bottle, BottleFile, BottleStable, FormulaUrls, SourceUrl, Versions, formula_token,
};
use zb_core::{BuildPlan, Formula, KegOnly, resolve_closure, select_bottle_for_platform};
use zb_io::network::suggest::rank_formula_suggestions;
use zb_io::validate_privileged_path;

fn formula(name: &str, deps: &[String], build_deps: &[String]) -> Formula {
    let mut files = BTreeMap::new();
    files.insert(
        "arm64_sequoia".to_string(),
        BottleFile {
            url: format!("https://example.invalid/{name}.arm64_sequoia.bottle.tar.gz"),
            sha256: "d".repeat(64),
        },
    );
    files.insert(
        "sequoia".to_string(),
        BottleFile {
            url: format!("https://example.invalid/{name}.sequoia.bottle.tar.gz"),
            sha256: "e".repeat(64),
        },
    );
    files.insert(
        "x86_64_linux".to_string(),
        BottleFile {
            url: format!("https://example.invalid/{name}.x86_64_linux.bottle.tar.gz"),
            sha256: "b".repeat(64),
        },
    );
    files.insert(
        "all".to_string(),
        BottleFile {
            url: format!("https://example.invalid/{name}.all.bottle.tar.gz"),
            sha256: "a".repeat(64),
        },
    );

    Formula {
        name: name.to_string(),
        aliases: Vec::new(),
        versions: Versions {
            stable: "1.0.0".to_string(),
        },
        dependencies: deps.to_vec(),
        bottle: Bottle {
            stable: BottleStable { files, rebuild: 0 },
        },
        revision: 0,
        keg_only: KegOnly::default(),
        keg_only_reason: None,
        build_dependencies: build_deps.to_vec(),
        homepage: None,
        urls: Some(FormulaUrls {
            stable: Some(SourceUrl {
                url: format!("https://example.invalid/{name}-1.0.0.tar.gz"),
                checksum: Some("c".repeat(64)),
                tag: None,
                revision: None,
            }),
            head: None,
        }),
        ruby_source_path: Some(format!("Formula/{}/{name}.rb", &name[..1])),
        ruby_source_checksum: None,
        uses_from_macos: Vec::new(),
        requirements: Vec::new(),
        variations: None,
    }
}

fn formula_graph(count: usize, deps_per_formula: usize) -> BTreeMap<String, Formula> {
    let mut formulas = BTreeMap::new();
    for idx in 0..count {
        let name = format!("pkg-{idx:04}");
        let deps = (1..=deps_per_formula)
            .filter_map(|offset| idx.checked_sub(offset))
            .map(|dep_idx| format!("pkg-{dep_idx:04}"))
            .collect::<Vec<_>>();
        let build_deps = match idx % 4 {
            0 => vec!["cmake".to_string()],
            1 => vec!["meson".to_string()],
            _ => Vec::new(),
        };
        formulas.insert(name.clone(), formula(&name, &deps, &build_deps));
    }
    formulas
}

fn formula_candidates(count: usize) -> Vec<String> {
    (0..count)
        .map(|idx| match idx % 6 {
            0 => format!("python-{idx}"),
            1 => format!("pyenv-{idx}"),
            2 => format!("ripgrep-{idx}"),
            3 => format!("openssl@3-{idx}"),
            4 => format!("pkgconf-{idx}"),
            _ => format!("zerobrew-test-formula-{idx}"),
        })
        .collect()
}

fn bench_core_formula(c: &mut Criterion) {
    let mut group = c.benchmark_group("zb_core::formula");

    for input in ["wget", "hashicorp/tap/terraform", "homebrew/core/openssl@3"] {
        group.bench_with_input(
            BenchmarkId::new("formula_token", input),
            input,
            |b, input| {
                b.iter(|| formula_token(black_box(input)));
            },
        );
    }

    let formula = formula("openssl@3", &[], &["cmake".to_string()]);
    group.bench_function("select_bottle_for_platform", |b| {
        b.iter(|| select_bottle_for_platform(black_box(&formula), black_box(Some(15))).unwrap());
    });

    group.finish();
}

fn bench_core_resolve(c: &mut Criterion) {
    let mut group = c.benchmark_group("zb_core::resolve_closure");
    for (count, deps_per_formula) in [(64, 2), (512, 4), (2_048, 6)] {
        let formulas = formula_graph(count, deps_per_formula);
        let roots = vec![format!("pkg-{:04}", count - 1)];
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{count}_formulas")),
            &(&roots, &formulas),
            |b, (roots, formulas)| {
                b.iter(|| resolve_closure(black_box(roots), black_box(formulas)).unwrap());
            },
        );
    }
    group.finish();
}

fn bench_core_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("zb_core::BuildPlan");
    let prefix = PathBuf::from("/opt/zerobrew");

    for build_deps in [
        Vec::<String>::new(),
        vec!["cmake".to_string()],
        vec!["meson".to_string(), "pkgconf".to_string()],
    ] {
        let formula = formula("libexample", &["zlib".to_string()], &build_deps);
        let label = if build_deps.is_empty() {
            "tarball"
        } else {
            build_deps[0].as_str()
        };
        group.bench_with_input(
            BenchmarkId::new("from_formula", label),
            &formula,
            |b, formula| {
                b.iter(|| BuildPlan::from_formula(black_box(formula), black_box(&prefix)).unwrap());
            },
        );
    }

    group.finish();
}

fn bench_io_suggestions(c: &mut Criterion) {
    let mut group = c.benchmark_group("zb_io::rank_formula_suggestions");
    for candidate_count in [64, 1_024, 8_192] {
        let candidates = formula_candidates(candidate_count);
        group.throughput(Throughput::Elements(candidate_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{candidate_count}_candidates")),
            &candidates,
            |b, candidates| {
                b.iter(|| {
                    rank_formula_suggestions(
                        black_box("pythn"),
                        black_box(candidates),
                        black_box(10),
                    )
                });
            },
        );
    }
    group.finish();
}

fn bench_io_paths(c: &mut Criterion) {
    let mut group = c.benchmark_group("zb_io::validate_privileged_path");
    for path in [
        "/opt/zerobrew",
        "/opt/zerobrew/Cellar/openssl@3/3.6.0/bin/openssl",
        "/opt/zerobrew/Cellar/very-long-package-name-with-tools/1.2.3/share/man/man1/tool.1",
    ] {
        group.bench_with_input(BenchmarkId::from_parameter(path), path, |b, path| {
            b.iter(|| validate_privileged_path(black_box(Path::new(path))).unwrap());
        });
    }
    group.finish();
}

fn bench_cli_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("zb_cli::Cli::try_parse_from");
    for args in [
        vec!["zb", "list"],
        vec![
            "zb",
            "--concurrency",
            "32",
            "install",
            "openssl@3",
            "ripgrep",
        ],
        vec![
            "zb",
            "-vv",
            "upgrade",
            "--dry-run",
            "git",
            "node",
            "python@3.14",
        ],
    ] {
        let label = args.join(" ");
        group.bench_with_input(BenchmarkId::from_parameter(label), &args, |b, args| {
            b.iter(|| Cli::try_parse_from(black_box(args)).unwrap());
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_core_formula,
    bench_core_resolve,
    bench_core_build,
    bench_io_suggestions,
    bench_io_paths,
    bench_cli_parse
);
criterion_main!(benches);
