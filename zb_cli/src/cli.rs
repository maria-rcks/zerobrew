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
    fn parses_commands_quiet() {
        let cli = Cli::try_parse_from(["zb", "commands", "--quiet"]).unwrap();
        assert!(matches!(
            cli.command,
            super::Commands::Commands {
                quiet: true,
                include_aliases: false
            }
        ));
    }

    #[test]
    fn parses_shellenv_shell_name() {
        let cli = Cli::try_parse_from(["zb", "shellenv", "fish"]).unwrap();
        assert!(matches!(
            cli.command,
            super::Commands::Shellenv { shell: Some(shell) } if shell == "fish"
        ));
    }

    #[test]
    fn parses_link_formula_names() {
        let cli = Cli::try_parse_from(["zb", "link", "foo", "bar"]).unwrap();
        assert!(matches!(
            cli.command,
            super::Commands::Link { formulas } if formulas == vec!["foo".to_string(), "bar".to_string()]
        ));
    }

    #[test]
    fn parses_unlink_formula_names() {
        let cli = Cli::try_parse_from(["zb", "unlink", "foo"]).unwrap();
        assert!(matches!(
            cli.command,
            super::Commands::Unlink { formulas } if formulas == vec!["foo".to_string()]
        ));
    }

    #[test]
    fn parses_search_text() {
        let cli = Cli::try_parse_from(["zb", "search", "rip", "grep"]).unwrap();
        assert!(matches!(
            cli.command,
            super::Commands::Search { text, .. } if text == vec!["rip".to_string(), "grep".to_string()]
        ));
    }

    #[test]
    fn parses_reinstall_formula_names() {
        let cli = Cli::try_parse_from(["zb", "reinstall", "foo"]).unwrap();
        assert!(matches!(
            cli.command,
            super::Commands::Reinstall { formulas, .. } if formulas == vec!["foo".to_string()]
        ));
    }

    #[test]
    fn parses_upgrade_without_names() {
        let cli = Cli::try_parse_from(["zb", "upgrade", "--dry-run"]).unwrap();
        assert!(matches!(
            cli.command,
            super::Commands::Upgrade {
                formulas,
                dry_run: true,
                ..
            } if formulas.is_empty()
        ));
    }

    #[test]
    fn parses_maintenance_commands() {
        assert!(matches!(
            Cli::try_parse_from(["zb", "cleanup"]).unwrap().command,
            super::Commands::Cleanup
        ));
        assert!(matches!(
            Cli::try_parse_from(["zb", "autoremove"]).unwrap().command,
            super::Commands::Autoremove
        ));
        assert!(matches!(
            Cli::try_parse_from(["zb", "leaves"]).unwrap().command,
            super::Commands::Leaves
        ));
        assert!(matches!(
            Cli::try_parse_from(["zb", "config"]).unwrap().command,
            super::Commands::Config
        ));
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
    #[command(visible_alias = "b")]
    Bundle {
        #[command(subcommand)]
        command: Option<BundleCommands>,
    },
    Autoremove,
    Cleanup,
    Config,
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
    Reinstall {
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
    Upgrade {
        #[arg(num_args = 0..)]
        formulas: Vec<String>,
        #[arg(long, short = 'n')]
        dry_run: bool,
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
    Link {
        #[arg(required = true, num_args = 1..)]
        formulas: Vec<String>,
    },
    Unlink {
        #[arg(required = true, num_args = 1..)]
        formulas: Vec<String>,
    },
    Leaves,
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
    Commands {
        #[arg(long, short = 'q')]
        quiet: bool,
        #[arg(long, requires = "quiet")]
        include_aliases: bool,
    },
    Shellenv {
        shell: Option<String>,
    },
    Search {
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
        #[arg(long, conflicts_with = "cask")]
        formula: bool,
        #[arg(long, conflicts_with = "formula")]
        cask: bool,
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
    #[command(visible_aliases = ["i", "add"])]
    Install {
        #[arg(long, short = 'f', value_name = "FILE", default_value = "Brewfile")]
        file: PathBuf,
        #[arg(long)]
        no_link: bool,
    },
    #[command(visible_alias = "d")]
    Dump {
        #[arg(long, short = 'f', value_name = "FILE", default_value = "Brewfile")]
        file: PathBuf,
        #[arg(long)]
        force: bool,
    },
}
