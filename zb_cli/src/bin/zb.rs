use clap::Parser;
use console::style;
use zb_cli::{
    cli::{Cli, Commands},
    commands,
    init::ensure_init,
    logging,
    ui::Ui,
    utils::get_root_path,
};
use zb_io::create_installer;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    logging::init(cli.verbose, cli.quiet);

    if let Err(e) = run(cli).await {
        eprintln!("{} {}", style("error:").red().bold(), e);
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), zb_core::Error> {
    let mut ui = Ui::new();

    if let Commands::Completion { shell } = cli.command {
        return commands::completion::execute(shell);
    }

    let root = get_root_path(cli.root);
    let prefix = cli.prefix.unwrap_or_else(|| {
        // On macOS, Mach-O binaries have fixed-size path fields so the prefix
        // must be no longer than the original Homebrew prefix (/opt/homebrew = 13 chars).
        // Using root directly (/opt/zerobrew = 13 chars) keeps us within that limit.
        if cfg!(target_os = "macos") {
            root.clone()
        } else {
            root.join("prefix")
        }
    });

    if let Commands::Init { no_modify_path } = cli.command {
        return commands::init::execute(&root, &prefix, no_modify_path, &mut ui);
    }

    if let Commands::Shellenv { shell } = cli.command {
        return commands::shellenv::execute(&root, &prefix, shell);
    }

    if let Commands::Commands {
        quiet,
        include_aliases,
    } = cli.command
    {
        return commands::command_list::execute(quiet, include_aliases);
    }

    if !matches!(cli.command, Commands::Reset { .. }) {
        ensure_init(&root, &prefix, cli.auto_init, &mut ui)?;
    }

    let mut installer = create_installer(&root, &prefix, cli.concurrency)?;

    match cli.command {
        Commands::Init { .. } => unreachable!(),
        Commands::Completion { .. } => unreachable!(),
        Commands::Commands { .. } => unreachable!(),
        Commands::Shellenv { .. } => unreachable!(),
        Commands::Install {
            formulas,
            no_link,
            build_from_source,
            cask,
            formula,
            appdir,
            fontdir,
            appimagedir,
            no_binaries,
            force,
        } => {
            commands::install::execute(
                &mut installer,
                commands::install::InstallRequest {
                    formulas,
                    no_link,
                    build_from_source,
                    cask,
                    formula,
                    appdir,
                    fontdir,
                    appimagedir,
                    no_binaries,
                    force,
                },
                &mut ui,
            )
            .await
        }
        Commands::Bundle { command } => {
            commands::bundle::execute(&mut installer, command, &mut ui).await
        }
        Commands::Autoremove | Commands::Cleanup => commands::gc::execute(&mut installer),
        Commands::Config => commands::config::execute(&root, &prefix),
        Commands::Uninstall {
            formulas,
            all,
            cask,
            formula,
        } => commands::uninstall::execute(&mut installer, formulas, all, cask, formula, &mut ui),
        Commands::Reinstall {
            formulas,
            no_link,
            build_from_source,
            cask,
            formula,
            appdir,
            fontdir,
            appimagedir,
            no_binaries,
            force,
        } => {
            commands::reinstall::execute(
                &mut installer,
                commands::reinstall::ReinstallRequest {
                    formulas,
                    no_link,
                    build_from_source,
                    cask,
                    formula,
                    appdir,
                    fontdir,
                    appimagedir,
                    no_binaries,
                    force,
                },
                &mut ui,
            )
            .await
        }
        Commands::Migrate { yes, force } => {
            commands::migrate::execute(&mut installer, yes, force, &mut ui).await
        }
        Commands::Link { formulas } => commands::link::execute(&mut installer, formulas, &mut ui),
        Commands::Unlink { formulas } => {
            commands::unlink::execute(&mut installer, formulas, &mut ui)
        }
        Commands::Doctor { repair } => commands::doctor::execute(&mut installer, repair, &mut ui),
        Commands::Leaves => commands::leaves::execute(&mut installer).await,
        Commands::List => commands::list::execute(&mut installer),
        Commands::Info { formula } => commands::info::execute(&mut installer, formula),
        Commands::Gc => commands::gc::execute(&mut installer),
        Commands::Update => commands::update::execute(&mut installer),
        Commands::Outdated { json } => {
            commands::outdated::execute(&mut installer, cli.quiet, cli.verbose > 0, json).await
        }
        Commands::Upgrade {
            formulas,
            dry_run,
            no_link,
            build_from_source,
            cask,
            formula,
            appdir,
            fontdir,
            appimagedir,
            no_binaries,
            force,
        } => {
            commands::upgrade::execute(
                &mut installer,
                commands::upgrade::UpgradeRequest {
                    formulas,
                    dry_run,
                    no_link,
                    build_from_source,
                    cask,
                    formula,
                    appdir,
                    fontdir,
                    appimagedir,
                    no_binaries,
                    force,
                },
                &mut ui,
            )
            .await
        }
        Commands::Search {
            text,
            formula,
            cask,
        } => commands::search::execute(&mut installer, text, formula, cask).await,
        Commands::Reset { yes } => commands::reset::execute(&root, &prefix, yes, &mut ui),
        Commands::Run { formula, args } => {
            commands::run::execute(&mut installer, formula, args).await
        }
    }
}
