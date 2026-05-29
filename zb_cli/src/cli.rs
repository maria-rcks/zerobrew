use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "zb")]
#[command(about = "Zerobrew - A fast Homebrew-compatible package installer")]
#[command(version)]
pub struct Cli {
    #[arg(long, env = "ZEROBREW_ROOT", help = "Path to zerobrew data directory")]
    pub root: Option<PathBuf>,

    #[arg(long, env = "ZEROBREW_PREFIX", help = "Path to Homebrew-style prefix")]
    pub prefix: Option<PathBuf>,

    #[arg(
        long,
        default_value = "20",
        value_parser = parse_concurrency,
        help = "Number of concurrent download threads"
    )]
    pub concurrency: usize,

    #[arg(
        long = "auto-init",
        global = true,
        env = "ZEROBREW_AUTO_INIT",
        help = "Automatically initialize without prompting"
    )]
    pub auto_init: bool,

    #[arg(
        long,
        short = 'v',
        global = true,
        action = clap::ArgAction::Count,
        help = "Increase output verbosity"
    )]
    pub verbose: u8,

    #[arg(
        long,
        short = 'q',
        global = true,
        conflicts_with = "verbose",
        help = "Suppress output, except for errors"
    )]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Commands,
}

fn parse_concurrency(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("invalid value '{}': expected a positive integer", value))?;
    if parsed == 0 {
        return Err("concurrency must be at least 1".to_string());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn accepts_positive_concurrency() {
        let cli = Cli::try_parse_from(["zb", "--concurrency", "4", "list"]).unwrap();
        assert_eq!(cli.concurrency, 4);
    }

    #[test]
    fn rejects_zero_concurrency() {
        let result = Cli::try_parse_from(["zb", "--concurrency", "0", "list"]);
        assert!(result.is_err());
        let err = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("at least 1"));
    }

    #[test]
    fn accepts_verbose_levels() {
        let cli = Cli::try_parse_from(["zb", "-vv", "list"]).unwrap();
        assert_eq!(cli.verbose, 2);
        assert!(!cli.quiet);
    }

    #[test]
    fn rejects_quiet_with_verbose() {
        let result = Cli::try_parse_from(["zb", "-v", "-q", "list"]);
        assert!(result.is_err());
    }

    #[test]
    fn outdated_quiet_and_verbose_conflict() {
        let result = Cli::try_parse_from(["zb", "outdated", "--quiet", "--verbose"]);
        assert!(result.is_err());
    }

    #[test]
    fn outdated_quiet_and_json_conflict() {
        let result = Cli::try_parse_from(["zb", "outdated", "--quiet", "--json"]);
        assert!(result.is_err());
    }

    #[test]
    fn outdated_verbose_and_json_conflict() {
        let result = Cli::try_parse_from(["zb", "outdated", "--verbose", "--json"]);
        assert!(result.is_err());
    }

    #[test]
    fn list_accepts_common_homebrew_filter_flags() {
        let cli = Cli::try_parse_from([
            "zb",
            "list",
            "--formula",
            "--versions",
            "--json",
            "--pinned",
            "jq",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::List {
                formulas,
                formula: true,
                cask: false,
                versions: true,
                json: true,
                pinned: true,
            } if formulas == vec!["jq"]
        ));
    }

    #[test]
    fn list_accepts_cask_filter_flag() {
        let cli = Cli::try_parse_from(["zb", "ls", "--cask"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::List {
                formula: false,
                cask: true,
                ..
            }
        ));
    }

    #[test]
    fn list_rejects_formula_with_cask() {
        let result = Cli::try_parse_from(["zb", "list", "--formula", "--cask"]);
        assert!(result.is_err());
    }

    #[test]
    fn deps_accepts_common_homebrew_flags() {
        let cli = Cli::try_parse_from([
            "zb",
            "deps",
            "--include-build",
            "--include-test",
            "--skip-recommended",
            "--tree",
            "--prune",
            "--missing",
            "--eval-all",
            "--recursive",
            "jq",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::Deps {
                formulas,
                include_build: true,
                include_test: true,
                skip_recommended: true,
                tree: true,
                prune: true,
                missing: true,
                eval_all: true,
                recursive: true,
            } if formulas == vec!["jq"]
        ));
    }

    #[test]
    fn uses_accepts_common_homebrew_flags() {
        let cli = Cli::try_parse_from([
            "zb",
            "uses",
            "--eval-all",
            "--include-build",
            "--include-optional",
            "--include-test",
            "--missing",
            "--recursive",
            "openssl@3",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::Uses {
                formulas,
                eval_all: true,
                include_build: true,
                include_optional: true,
                include_test: true,
                missing: true,
                recursive: true,
            } if formulas == vec!["openssl@3"]
        ));
    }

    #[test]
    fn info_aliases_accept_common_homebrew_flags() {
        let aliases = ["show", "cat", "desc", "home", "homepage"];
        for alias in aliases {
            let cli = Cli::try_parse_from([
                "zb",
                alias,
                "--installed",
                "--eval-all",
                "--analytics",
                "--json",
                "jq",
            ])
            .unwrap_or_else(|err| panic!("{alias} failed to parse: {err}"));
            assert!(matches!(
                cli.command,
                Commands::Info {
                    formula,
                    installed: true,
                    eval_all: true,
                    analytics: true,
                    json: true,
                    show_versions: false,
                } if formula == "jq"
            ));
        }
    }

    #[test]
    fn install_reinstall_and_upgrade_accept_common_homebrew_flags() {
        let commands = ["install", "reinstall", "upgrade"];
        for command in commands {
            let cli = Cli::try_parse_from([
                "zb",
                command,
                "--force-bottle",
                "--ignore-dependencies",
                "--only-dependencies",
                "jq",
            ])
            .unwrap_or_else(|err| panic!("{command} failed to parse: {err}"));
            assert!(matches!(
                cli.command,
                Commands::Install {
                    force_bottle: true,
                    ignore_dependencies: true,
                    only_dependencies: true,
                    ..
                } | Commands::Reinstall {
                    force_bottle: true,
                    ignore_dependencies: true,
                    only_dependencies: true,
                    ..
                } | Commands::Upgrade {
                    force_bottle: true,
                    ignore_dependencies: true,
                    only_dependencies: true,
                    ..
                }
            ));
        }
    }

    #[test]
    fn install_reinstall_and_upgrade_accept_single_common_homebrew_flag() {
        let flags = [
            "--force-bottle",
            "--ignore-dependencies",
            "--only-dependencies",
        ];
        for flag in flags {
            for command in ["install", "reinstall", "upgrade"] {
                Cli::try_parse_from(["zb", command, flag, "jq"])
                    .unwrap_or_else(|err| panic!("{command} {flag} failed to parse: {err}"));
            }
        }
    }

    #[test]
    fn info_accepts_common_homebrew_output_flags() {
        let cli = Cli::try_parse_from(["zb", "info", "--versions", "jq"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Info {
                show_versions: true,
                ..
            }
        ));
    }

    #[test]
    fn search_accepts_common_homebrew_filter_flags() {
        let cli = Cli::try_parse_from([
            "zb",
            "search",
            "--installed",
            "--eval-all",
            "--json",
            "--desc",
            "--name",
            "--all",
            "json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::Search {
                installed: true,
                eval_all: true,
                json: true,
                desc: true,
                name: true,
                all: true,
                ..
            }
        ));
    }

    #[test]
    fn outdated_accepts_common_homebrew_filter_flags() {
        let cli = Cli::try_parse_from([
            "zb",
            "outdated",
            "--formula",
            "--cask",
            "--fetch-head",
            "--pinned",
            "--unpinned",
            "--greedy",
            "--greedy-auto-updates",
            "--greedy-latest",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::Outdated {
                formula: true,
                cask: true,
                fetch_head: true,
                pinned: true,
                unpinned: true,
                greedy: true,
                greedy_auto_updates: true,
                greedy_latest: true,
                ..
            }
        ));
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Install formulas and casks
    #[command(visible_aliases = ["i", "add"])]
    Install {
        #[arg(required = true, num_args = 1.., help = "Packages to install")]
        formulas: Vec<String>,
        #[arg(long, help = "Do not create symlinks after installation")]
        no_link: bool,
        #[arg(long, short = 's', help = "Build from source instead of using bottles")]
        build_from_source: bool,
        #[arg(long, help = "Install from bottles only")]
        force_bottle: bool,
        #[arg(long, help = "Ignore dependencies when installing formulas")]
        ignore_dependencies: bool,
        #[arg(long, help = "Install only missing dependencies")]
        only_dependencies: bool,
        #[arg(long, conflicts_with = "formula", help = "Treat packages as casks")]
        cask: bool,
        #[arg(long, conflicts_with = "cask", help = "Treat packages as formulas")]
        formula: bool,
        #[arg(long, help = "Directory for installed app bundles")]
        appdir: Option<PathBuf>,
        #[arg(long, help = "Directory for installed fonts")]
        fontdir: Option<PathBuf>,
        #[arg(long, help = "Directory for installed AppImages")]
        appimagedir: Option<PathBuf>,
        #[arg(long, help = "Skip cask binary linking")]
        no_binaries: bool,
        #[arg(long, help = "Overwrite existing cask artifacts")]
        force: bool,
    },
    /// Install or dump packages from a Brewfile
    #[command(visible_alias = "b")]
    Bundle {
        #[command(subcommand)]
        command: Option<BundleCommands>,
    },
    /// Remove packages that are no longer needed
    #[command(visible_alias = "prune")]
    Autoremove,
    /// Remove cached downloads and build artifacts
    #[command(visible_alias = "clean")]
    Cleanup,
    /// Show zerobrew configuration
    #[command(visible_alias = "cfg")]
    Config,
    /// Uninstall formulas and casks
    #[command(visible_aliases = ["rm", "remove"])]
    Uninstall {
        #[arg(required_unless_present = "all", num_args = 1.., help = "Packages to uninstall")]
        formulas: Vec<String>,
        #[arg(long, help = "Uninstall all installed packages")]
        all: bool,
        #[arg(long, conflicts_with = "formula", help = "Treat packages as casks")]
        cask: bool,
        #[arg(long, conflicts_with = "cask", help = "Treat packages as formulas")]
        formula: bool,
    },
    /// Migrate packages from Homebrew
    Migrate {
        #[arg(long, short = 'y', help = "Skip confirmation prompts")]
        yes: bool,
        #[arg(long, help = "Force uninstall from Homebrew even if errors occur")]
        force: bool,
    },
    /// Reinstall formulas and casks
    #[command(visible_alias = "re")]
    Reinstall {
        #[arg(required = true, num_args = 1.., help = "Packages to reinstall")]
        formulas: Vec<String>,
        #[arg(long, help = "Do not create symlinks after installation")]
        no_link: bool,
        #[arg(long, short = 's', help = "Build from source instead of using bottles")]
        build_from_source: bool,
        #[arg(long, help = "Install from bottles only")]
        force_bottle: bool,
        #[arg(long, help = "Ignore dependencies when installing formulas")]
        ignore_dependencies: bool,
        #[arg(long, help = "Install only missing dependencies")]
        only_dependencies: bool,
        #[arg(long, conflicts_with = "formula", help = "Treat packages as casks")]
        cask: bool,
        #[arg(long, conflicts_with = "cask", help = "Treat packages as formulas")]
        formula: bool,
        #[arg(long, help = "Directory for installed app bundles")]
        appdir: Option<PathBuf>,
        #[arg(long, help = "Directory for installed fonts")]
        fontdir: Option<PathBuf>,
        #[arg(long, help = "Directory for installed AppImages")]
        appimagedir: Option<PathBuf>,
        #[arg(long, help = "Skip cask binary linking")]
        no_binaries: bool,
        #[arg(long, help = "Overwrite existing cask artifacts")]
        force: bool,
    },
    /// Upgrade installed packages
    #[command(visible_alias = "ug")]
    Upgrade {
        #[arg(num_args = 0.., help = "Packages to upgrade")]
        formulas: Vec<String>,
        #[arg(long, short = 'n', help = "Show what would be upgraded")]
        dry_run: bool,
        #[arg(long, help = "Do not create symlinks after installation")]
        no_link: bool,
        #[arg(long, short = 's', help = "Build from source instead of using bottles")]
        build_from_source: bool,
        #[arg(long, help = "Install from bottles only")]
        force_bottle: bool,
        #[arg(long, help = "Ignore dependencies when installing formulas")]
        ignore_dependencies: bool,
        #[arg(long, help = "Install only missing dependencies")]
        only_dependencies: bool,
        #[arg(long, conflicts_with = "formula", help = "Treat packages as casks")]
        cask: bool,
        #[arg(long, conflicts_with = "cask", help = "Treat packages as formulas")]
        formula: bool,
        #[arg(long, help = "Directory for installed app bundles")]
        appdir: Option<PathBuf>,
        #[arg(long, help = "Directory for installed fonts")]
        fontdir: Option<PathBuf>,
        #[arg(long, help = "Directory for installed AppImages")]
        appimagedir: Option<PathBuf>,
        #[arg(long, help = "Skip cask binary linking")]
        no_binaries: bool,
        #[arg(long, help = "Overwrite existing cask artifacts")]
        force: bool,
    },
    /// Link installed packages into the prefix
    #[command(visible_alias = "ln")]
    Link {
        #[arg(required = true, num_args = 1.., help = "Packages to link")]
        formulas: Vec<String>,
    },
    /// Remove package links from the prefix
    #[command(visible_alias = "unln")]
    Unlink {
        #[arg(required = true, num_args = 1.., help = "Packages to unlink")]
        formulas: Vec<String>,
    },
    /// List packages with no installed dependents
    #[command(visible_alias = "leaf")]
    Leaves,
    /// List installed packages
    #[command(visible_alias = "ls")]
    List {
        #[arg(num_args = 0.., help = "Packages to list")]
        formulas: Vec<String>,
        #[arg(long, conflicts_with = "cask", help = "List formulae only")]
        formula: bool,
        #[arg(long, conflicts_with = "formula", help = "List casks only")]
        cask: bool,
        #[arg(long, help = "Show installed package versions")]
        versions: bool,
        #[arg(long, help = "Output as JSON when supported")]
        json: bool,
        #[arg(long, help = "List pinned packages when supported")]
        pinned: bool,
    },
    /// Show dependencies for formulas
    Deps {
        #[arg(required = true, num_args = 1.., help = "Formula names to inspect")]
        formulas: Vec<String>,
        #[arg(long, help = "Include build dependencies when supported")]
        include_build: bool,
        #[arg(long, help = "Include test dependencies when supported")]
        include_test: bool,
        #[arg(long, help = "Skip recommended dependencies when supported")]
        skip_recommended: bool,
        #[arg(long, help = "Show dependencies as a tree when supported")]
        tree: bool,
        #[arg(
            long,
            help = "Prune repeated dependencies in tree output when supported"
        )]
        prune: bool,
        #[arg(long, help = "Only show missing dependencies when supported")]
        missing: bool,
        #[arg(long, help = "Also evaluate all formulae when supported")]
        eval_all: bool,
        #[arg(long, help = "Resolve dependencies recursively when supported")]
        recursive: bool,
    },
    /// Show formulas that depend on the given formulas
    Uses {
        #[arg(required = true, num_args = 1.., help = "Formula names to inspect")]
        formulas: Vec<String>,
        #[arg(long, help = "Also evaluate all formulae when supported")]
        eval_all: bool,
        #[arg(long, help = "Include build dependencies when finding dependents")]
        include_build: bool,
        #[arg(long, help = "Include optional dependencies when supported")]
        include_optional: bool,
        #[arg(long, help = "Include test dependencies when supported")]
        include_test: bool,
        #[arg(long, help = "Only show missing dependents when supported")]
        missing: bool,
        #[arg(long, help = "Resolve dependents recursively when supported")]
        recursive: bool,
    },
    /// Show information about a formula
    #[command(visible_aliases = ["show", "cat", "desc", "home", "homepage"])]
    Info {
        #[arg(help = "Name of the formula")]
        formula: String,
        #[arg(long, help = "Show installed formula information only")]
        installed: bool,
        #[arg(long, help = "Also evaluate all formulae when supported")]
        eval_all: bool,
        #[arg(long, help = "Show analytics information when supported")]
        analytics: bool,
        #[arg(long, help = "Output as JSON when supported")]
        json: bool,
        #[arg(long = "versions", help = "Show package versions when supported")]
        show_versions: bool,
    },
    /// Run diagnostics and optionally repair issues
    #[command(visible_alias = "check")]
    Doctor {
        #[arg(long, help = "Automatically repair detected issues")]
        repair: bool,
    },
    /// Remove unreferenced store entries
    Gc,
    /// Reset zerobrew data directories
    Reset {
        #[arg(long, short = 'y', help = "Skip confirmation prompts")]
        yes: bool,
    },
    /// Initialize zerobrew directories
    Init {
        #[arg(long, help = "Do not modify shell configuration files")]
        no_modify_path: bool,
    },
    /// Generate shell completions
    Completion {
        #[arg(value_enum, help = "Target shell for completions")]
        shell: clap_complete::shells::Shell,
    },
    /// List commands and aliases
    #[command(visible_alias = "cmds")]
    Commands {
        #[arg(long, short = 'q', help = "Only print command names")]
        quiet: bool,
        #[arg(long, requires = "quiet", help = "Include aliases in quiet output")]
        include_aliases: bool,
    },
    /// Print shell environment exports
    #[command(visible_alias = "env")]
    Shellenv {
        #[arg(help = "Shell syntax to emit")]
        shell: Option<String>,
    },
    /// Search formula and cask names
    #[command(visible_alias = "find")]
    Search {
        #[arg(required = true, num_args = 1.., help = "Search text")]
        text: Vec<String>,
        #[arg(long, conflicts_with = "cask", help = "Search formulas only")]
        formula: bool,
        #[arg(long, conflicts_with = "formula", help = "Search casks only")]
        cask: bool,
        #[arg(long, help = "Include installed formulae when supported")]
        installed: bool,
        #[arg(long, help = "Also evaluate all formulae when supported")]
        eval_all: bool,
        #[arg(long, help = "Output as JSON when supported")]
        json: bool,
        #[arg(long, help = "Show formula descriptions when supported")]
        desc: bool,
        #[arg(long, help = "Search package names when supported")]
        name: bool,
        #[arg(long, help = "Search all package metadata when supported")]
        all: bool,
    },
    /// Run an installed formula as a command
    #[command(disable_help_flag = true)]
    Run {
        #[arg(help = "Name of the formula to run")]
        formula: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Clear cached formula data
    #[command(visible_alias = "up")]
    Update,
    /// List installed packages with newer versions available
    #[command(visible_alias = "old")]
    Outdated {
        #[arg(long, conflicts_with_all = ["quiet", "verbose"], help = "Output as JSON")]
        json: bool,
        #[arg(long, help = "Include formulae when checking outdated packages")]
        formula: bool,
        #[arg(long, help = "Include casks when checking outdated packages")]
        cask: bool,
        #[arg(long, help = "Fetch the latest package metadata before checking")]
        fetch_head: bool,
        #[arg(long, help = "Include pinned packages when supported")]
        pinned: bool,
        #[arg(long, help = "Ignore pinned packages when supported")]
        unpinned: bool,
        #[arg(long, help = "Show outdated dependency information when supported")]
        greedy: bool,
        #[arg(long, help = "Show auto-updated casks when supported")]
        greedy_auto_updates: bool,
        #[arg(long, help = "Show latest-version casks when supported")]
        greedy_latest: bool,
    },
}

#[derive(Subcommand)]
pub enum BundleCommands {
    /// Install packages from a Brewfile
    #[command(visible_aliases = ["i", "add"])]
    Install {
        #[arg(
            long,
            short = 'f',
            value_name = "FILE",
            default_value = "Brewfile",
            help = "Path to the Brewfile"
        )]
        file: PathBuf,
        #[arg(long, help = "Do not create symlinks after installation")]
        no_link: bool,
    },
    /// Dump installed packages to a Brewfile
    #[command(visible_alias = "d")]
    Dump {
        #[arg(
            long,
            short = 'f',
            value_name = "FILE",
            default_value = "Brewfile",
            help = "Output file path"
        )]
        file: PathBuf,
        #[arg(long, help = "Overwrite existing file")]
        force: bool,
    },
}
