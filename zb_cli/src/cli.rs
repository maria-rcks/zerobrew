use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "zb")]
#[command(about = "Zerobrew - A fast Homebrew-compatible package installer")]
#[command(version)]
pub struct Cli {
    #[arg(long, env = "ZEROBREW_ROOT")]
    pub root: Option<PathBuf>,

    #[arg(long, env = "ZEROBREW_PREFIX")]
    pub prefix: Option<PathBuf>,

    #[arg(
        long,
        default_value = "20",
        value_parser = parse_concurrency
    )]
    pub concurrency: usize,

    #[arg(long = "auto-init", global = true, env = "ZEROBREW_AUTO_INIT")]
    pub auto_init: bool,

    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[arg(long, short = 'q', global = true, conflicts_with = "verbose")]
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
    use super::Cli;
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
    fn parses_install_aliases() {
        for alias in ["i", "add"] {
            let cli = Cli::try_parse_from(["zb", alias, "jq"]).unwrap();

            let super::Commands::Install { formulas, .. } = cli.command else {
                panic!("expected install command for alias {alias}");
            };
            assert_eq!(formulas, ["jq"]);
        }
    }

    #[test]
    fn parses_uninstall_aliases() {
        for alias in ["rm", "remove"] {
            let cli = Cli::try_parse_from(["zb", alias, "jq"]).unwrap();

            let super::Commands::Uninstall { formulas, .. } = cli.command else {
                panic!("expected uninstall command for alias {alias}");
            };
            assert_eq!(formulas, ["jq"]);
        }
    }

    #[test]
    fn parses_list_alias() {
        let cli = Cli::try_parse_from(["zb", "ls"]).unwrap();

        assert!(matches!(cli.command, super::Commands::List));
    }

    #[test]
    fn parses_info_alias() {
        let cli = Cli::try_parse_from(["zb", "show", "jq"]).unwrap();

        let super::Commands::Info { formula } = cli.command else {
            panic!("expected info command");
        };
        assert_eq!(formula, "jq");
    }

    #[test]
    fn parses_maintenance_aliases() {
        let cli = Cli::try_parse_from(["zb", "check"]).unwrap();
        assert!(matches!(cli.command, super::Commands::Doctor { .. }));

        for alias in ["clean", "cleanup"] {
            let cli = Cli::try_parse_from(["zb", alias]).unwrap();
            assert!(matches!(cli.command, super::Commands::Gc));
        }

        let cli = Cli::try_parse_from(["zb", "up"]).unwrap();
        assert!(matches!(cli.command, super::Commands::Update));

        let cli = Cli::try_parse_from(["zb", "old"]).unwrap();
        assert!(matches!(cli.command, super::Commands::Outdated { .. }));
    }
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(visible_aliases = ["i", "add"])]
    Install {
        #[arg(required = true, num_args = 1..)]
        formulas: Vec<String>,
        #[arg(long)]
        no_link: bool,
        #[arg(long, short = 's')]
        build_from_source: bool,
        #[arg(long, conflicts_with = "formula")]
        cask: bool,
        #[arg(long, conflicts_with = "cask")]
        formula: bool,
        #[arg(long)]
        appdir: Option<PathBuf>,
        #[arg(long)]
        fontdir: Option<PathBuf>,
        #[arg(long)]
        appimagedir: Option<PathBuf>,
        #[arg(long)]
        no_binaries: bool,
        #[arg(long)]
        force: bool,
    },
    Bundle {
        #[command(subcommand)]
        command: Option<BundleCommands>,
    },
    #[command(visible_aliases = ["rm", "remove"])]
    Uninstall {
        #[arg(required_unless_present = "all", num_args = 1..)]
        formulas: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long, conflicts_with = "formula")]
        cask: bool,
        #[arg(long, conflicts_with = "cask")]
        formula: bool,
    },
    Migrate {
        #[arg(long, short = 'y')]
        yes: bool,
        #[arg(long)]
        force: bool,
    },
    #[command(visible_alias = "ls")]
    List,
    #[command(visible_alias = "show")]
    Info { formula: String },
    #[command(visible_alias = "check")]
    Doctor {
        #[arg(long)]
        repair: bool,
    },
    #[command(visible_aliases = ["clean", "cleanup"])]
    Gc,
    Reset {
        #[arg(long, short = 'y')]
        yes: bool,
    },
    Init {
        #[arg(long)]
        no_modify_path: bool,
    },
    Completion {
        #[arg(value_enum)]
        shell: clap_complete::shells::Shell,
    },
    #[command(disable_help_flag = true)]
    Run {
        formula: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    #[command(visible_alias = "up")]
    Update,
    #[command(visible_alias = "old")]
    Outdated {
        /// Output as JSON
        #[arg(long, conflicts_with_all = ["quiet", "verbose"])]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum BundleCommands {
    Install {
        #[arg(long, short = 'f', value_name = "FILE", default_value = "Brewfile")]
        file: PathBuf,
        #[arg(long)]
        no_link: bool,
    },
    Dump {
        #[arg(long, short = 'f', value_name = "FILE", default_value = "Brewfile")]
        file: PathBuf,
        #[arg(long)]
        force: bool,
    },
}
