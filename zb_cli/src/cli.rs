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
    List,
    /// Show information about an installed package
    #[command(visible_alias = "show")]
    Info {
        #[arg(help = "Name of the installed package")]
        formula: String,
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
