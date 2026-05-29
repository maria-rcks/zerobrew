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
            force_bottle,
            ignore_dependencies,
            only_dependencies,
            cask,
            formula,
            appdir,
            fontdir,
            appimagedir,
            no_binaries,
            force,
        } => {
            warn_ignored_install_flags(
                force_bottle,
                ignore_dependencies,
                only_dependencies,
                &mut ui,
            )?;
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
            force_bottle,
            ignore_dependencies,
            only_dependencies,
            cask,
            formula,
            appdir,
            fontdir,
            appimagedir,
            no_binaries,
            force,
        } => {
            warn_ignored_install_flags(
                force_bottle,
                ignore_dependencies,
                only_dependencies,
                &mut ui,
            )?;
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
        Commands::List {
            formulas,
            formula: _,
            cask: _,
            versions,
            json,
            pinned: _,
        } => commands::list::execute(&mut installer, formulas, versions, json),
        Commands::Formulae { versions } => {
            commands::formulae::execute(&mut installer, versions).await
        }
        Commands::Casks => commands::casks::execute(&mut installer).await,
        Commands::Deps {
            formulas,
            include_build,
            include_test,
            skip_recommended,
            tree,
            prune,
            missing,
            eval_all,
            recursive,
        } => {
            warn_ignored_flags(
                &[
                    (include_test, "--include-test"),
                    (skip_recommended, "--skip-recommended"),
                    (tree, "--tree"),
                    (prune, "--prune"),
                    (missing, "--missing"),
                    (eval_all, "--eval-all"),
                    (recursive, "--recursive"),
                ],
                &mut ui,
            )?;
            commands::deps::execute(&mut installer, formulas, include_build).await
        }
        Commands::Uses {
            formulas,
            eval_all,
            include_build,
            include_optional,
            include_test,
            missing,
            recursive,
        } => {
            warn_ignored_flags(
                &[
                    (eval_all, "--eval-all"),
                    (include_optional, "--include-optional"),
                    (include_test, "--include-test"),
                    (missing, "--missing"),
                    (recursive, "--recursive"),
                ],
                &mut ui,
            )?;
            commands::uses::execute(&mut installer, formulas, include_build).await
        }
        Commands::Missing { formulas, hide } => {
            commands::missing::execute(&mut installer, formulas, hide).await
        }
        Commands::Info {
            formula,
            installed: _,
            eval_all: _,
            analytics: _,
            json: _,
            show_versions: _,
        } => commands::info::execute(&mut installer, formula),
        Commands::Gc => commands::gc::execute(&mut installer),
        Commands::Update => commands::update::execute(&mut installer),
        Commands::Outdated {
            json,
            formula: _,
            cask: _,
            fetch_head: _,
            pinned: _,
            unpinned: _,
            greedy: _,
            greedy_auto_updates: _,
            greedy_latest: _,
        } => commands::outdated::execute(&mut installer, cli.quiet, cli.verbose > 0, json).await,
        Commands::Pin {
            formulas,
            formula: _,
            cask: _,
        }
        | Commands::Unpin {
            formulas,
            formula: _,
            cask: _,
        } => commands::pin::execute(formulas),
        Commands::Options {
            formulas: _,
            compact: _,
            installed: _,
            eval_all: _,
            command: _,
        } => {
            commands::options::execute();
            Ok(())
        }
        Commands::Prefix {
            formulas,
            installed: _,
            unbrewed: _,
        } => commands::prefix::execute(&prefix, formulas, commands::prefix::PathKind::Prefix),
        Commands::Cellar { formulas } => {
            commands::prefix::execute(&prefix, formulas, commands::prefix::PathKind::Cellar)
        }
        Commands::Cat {
            formulas,
            formula: _,
            cask: _,
        } => commands::source::execute(&mut installer, formulas).await,
        Commands::Home {
            formulas,
            formula: _,
            cask: _,
        } => commands::home::execute(&mut installer, formulas).await,
        Commands::Upgrade {
            formulas,
            dry_run,
            no_link,
            build_from_source,
            force_bottle,
            ignore_dependencies,
            only_dependencies,
            cask,
            formula,
            appdir,
            fontdir,
            appimagedir,
            no_binaries,
            force,
        } => {
            warn_ignored_install_flags(
                force_bottle,
                ignore_dependencies,
                only_dependencies,
                &mut ui,
            )?;
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
            installed: _,
            eval_all: _,
            json: _,
            desc: _,
            name: _,
            all: _,
        } => commands::search::execute(&mut installer, text, formula, cask).await,
        Commands::Reset { yes } => commands::reset::execute(&root, &prefix, yes, &mut ui),
        Commands::Run { formula, args } => {
            commands::run::execute(&mut installer, formula, args).await
        }
    }
}

fn warn_ignored_install_flags(
    force_bottle: bool,
    ignore_dependencies: bool,
    only_dependencies: bool,
    ui: &mut zb_cli::ui::StdUi,
) -> Result<(), zb_core::Error> {
    warn_ignored_flags(
        &[
            (force_bottle, "--force-bottle"),
            (ignore_dependencies, "--ignore-dependencies"),
            (only_dependencies, "--only-dependencies"),
        ],
        ui,
    )
}

fn warn_ignored_flags(
    flags: &[(bool, &'static str)],
    ui: &mut zb_cli::ui::StdUi,
) -> Result<(), zb_core::Error> {
    let ignored: Vec<_> = flags
        .iter()
        .filter_map(|(enabled, flag)| enabled.then_some(*flag))
        .collect();

    if !ignored.is_empty() {
        ui.warn(format!(
            "{} accepted for Homebrew CLI compatibility but not applied yet",
            ignored.join(", ")
        ))
        .map_err(ui_error)?;
    }

    Ok(())
}

fn ui_error(err: std::io::Error) -> zb_core::Error {
    zb_core::Error::StoreCorruption {
        message: format!("failed to write CLI output: {err}"),
    }
}
