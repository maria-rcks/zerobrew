use zb_cli::{
    cli::{Cli, Commands},
    commands,
    init::ensure_init,
    logging,
    ui::{Ui, UiOptions},
    utils::get_root_path,
};
use zb_io::create_installer;

fn main() {
    // Restore the default SIGPIPE disposition so that piping data output
    // into a consumer that exits early (`zb list | head`) terminates this
    // process silently, like any other Unix CLI, instead of surfacing a
    // broken-pipe write error.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    // `process::exit` is called only here, after the async runtime and all
    // command state have been dropped cleanly.
    std::process::exit(run_main());
}

#[tokio::main]
async fn run_main() -> i32 {
    let cli = Cli::parse();
    let mut ui = Ui::from_options(UiOptions {
        quiet: cli.quiet,
        verbose: cli.verbose,
        color: cli.color,
    });
    logging::init(cli.verbose, cli.quiet, ui.multi_progress(), ui.color_err());

    let result = run(cli, &mut ui).await;
    ui.flush();
    match result {
        Ok(()) => 0,
        Err(e) => {
            ui.error(&e);
            ui.flush();
            e.exit_code()
        }
    }
}

async fn run(cli: Cli, ui: &mut Ui) -> Result<(), zb_core::Error> {
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
        return commands::init::execute(&root, &prefix, no_modify_path, ui);
    }

    if let Commands::Shellenv { shell } = cli.command {
        return commands::shellenv::execute(&root, &prefix, shell, ui);
    }

    if let Commands::Commands {
        quiet,
        include_aliases,
    } = cli.command
    {
        return commands::command_list::execute(quiet, include_aliases, ui);
    }

    if let Commands::Prefix {
        formulas,
        installed: _,
        unbrewed: _,
    } = cli.command
    {
        return commands::prefix::execute(
            &prefix,
            formulas,
            commands::prefix::PathKind::Prefix,
            ui,
        );
    }

    if let Commands::Cellar { formulas } = cli.command {
        return commands::prefix::execute(
            &prefix,
            formulas,
            commands::prefix::PathKind::Cellar,
            ui,
        );
    }

    if !matches!(cli.command, Commands::Reset { .. }) {
        ensure_init(&root, &prefix, cli.auto_init, ui)?;
    }

    let mut installer = create_installer(&root, &prefix, cli.concurrency)?;

    match cli.command {
        Commands::Init { .. } => unreachable!(),
        Commands::Completion { .. } => unreachable!(),
        Commands::Commands { .. } => unreachable!(),
        Commands::Shellenv { .. } => unreachable!(),
        Commands::Prefix { .. } => unreachable!(),
        Commands::Cellar { .. } => unreachable!(),
        Commands::Install {
            formulas,
            no_link,
            build_from_source,
            ignore_dependencies,
            only_dependencies,
            ask,
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
                    ignore_dependencies,
                    only_dependencies,
                    ask,
                    cask,
                    formula,
                    appdir,
                    fontdir,
                    appimagedir,
                    no_binaries,
                    force,
                },
                ui,
            )
            .await
        }
        Commands::Bundle { command } => {
            commands::bundle::execute(&mut installer, command, ui).await
        }
        Commands::Autoremove | Commands::Cleanup => commands::gc::execute(&mut installer, ui),
        Commands::Config => commands::config::execute(&root, &prefix, ui),
        Commands::Uninstall {
            formulas,
            all,
            cask,
            formula,
        } => commands::uninstall::execute(&mut installer, formulas, all, cask, formula, ui),
        Commands::Reinstall {
            formulas,
            no_link,
            build_from_source,
            ignore_dependencies,
            only_dependencies,
            ask,
            cask,
            formula,
            appdir,
            fontdir,
            appimagedir,
            no_binaries,
            force,
        } => {
            warn_ignored_install_flags(ignore_dependencies, only_dependencies, false, ui);
            commands::reinstall::execute(
                &mut installer,
                commands::reinstall::ReinstallRequest {
                    formulas,
                    no_link,
                    build_from_source,
                    ask,
                    cask,
                    formula,
                    appdir,
                    fontdir,
                    appimagedir,
                    no_binaries,
                    force,
                },
                ui,
            )
            .await
        }
        Commands::Migrate { yes, force } => {
            commands::migrate::execute(&mut installer, yes, force, ui).await
        }
        Commands::Link { formulas } => commands::link::execute(&mut installer, formulas, ui),
        Commands::Unlink { formulas } => commands::unlink::execute(&mut installer, formulas, ui),
        Commands::Doctor { repair } => commands::doctor::execute(&mut installer, repair, ui),
        Commands::Leaves => commands::leaves::execute(&mut installer, ui).await,
        Commands::List {
            formulas,
            formula: _,
            cask: _,
            versions,
            json,
            pinned: _,
        } => commands::list::execute(&mut installer, formulas, versions, json, ui),
        Commands::Formulae { versions } => {
            commands::formulae::execute(&mut installer, versions, ui).await
        }
        Commands::Casks => commands::casks::execute(&mut installer, ui).await,
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
                ui,
            );
            commands::deps::execute(&mut installer, formulas, include_build, ui).await
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
                ui,
            );
            commands::uses::execute(&mut installer, formulas, include_build, ui).await
        }
        Commands::Missing { formulas, hide } => {
            commands::missing::execute(&mut installer, formulas, hide, ui).await
        }
        Commands::Info {
            formula,
            installed: _,
            eval_all: _,
            analytics: _,
            json,
            show_versions,
        } => {
            warn_ignored_flags(&[(json, "--json")], ui);
            commands::info::execute(&mut installer, formula, show_versions, ui).await
        }
        Commands::Gc => commands::gc::execute(&mut installer, ui),
        Commands::Update => commands::update::execute(&mut installer, ui),
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
        } => commands::outdated::execute(&mut installer, json, ui).await,
        Commands::Options {
            formulas,
            compact,
            installed,
            eval_all,
            command,
        } => {
            warn_ignored_flags(&[(installed, "--installed"), (eval_all, "--eval-all")], ui);
            commands::options::execute(&mut installer, &root, formulas, compact, command, ui).await
        }
        Commands::Cat {
            formulas,
            formula: _,
            cask: _,
        } => commands::source::execute(&mut installer, formulas, ui).await,
        Commands::Edit {
            formulas,
            formula: _,
            cask,
            print_path,
        } => commands::edit::execute(&mut installer, &root, formulas, cask, print_path, ui).await,
        Commands::Home {
            formulas,
            formula: _,
            cask: _,
        } => commands::home::execute(&mut installer, formulas, ui).await,
        Commands::Upgrade {
            formulas,
            dry_run,
            no_link,
            build_from_source,
            ignore_dependencies,
            only_dependencies,
            ask,
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
                    ignore_dependencies,
                    only_dependencies,
                    ask,
                    cask,
                    formula,
                    appdir,
                    fontdir,
                    appimagedir,
                    no_binaries,
                    force,
                },
                ui,
            )
            .await
        }
        Commands::Search {
            text,
            formula,
            cask,
            installed: _,
            eval_all: _,
            json,
            desc,
            name,
            all,
        } => {
            warn_ignored_flags(&[(json, "--json")], ui);
            commands::search::execute(
                &mut installer,
                commands::search::SearchRequest {
                    text,
                    formula,
                    cask,
                    name,
                    all,
                    desc,
                },
                ui,
            )
            .await
        }
        Commands::Reset { yes } => commands::reset::execute(&root, &prefix, yes, ui),
        Commands::Run { formula, args } => {
            commands::run::execute(&mut installer, formula, args, ui).await
        }
    }
}

fn warn_ignored_install_flags(
    ignore_dependencies: bool,
    only_dependencies: bool,
    ask: bool,
    ui: &mut Ui,
) {
    warn_ignored_flags(
        &[
            (ignore_dependencies, "--ignore-dependencies"),
            (only_dependencies, "--only-dependencies"),
            (ask, "--ask"),
        ],
        ui,
    )
}

fn warn_ignored_flags(flags: &[(bool, &'static str)], ui: &mut Ui) {
    let ignored: Vec<_> = flags
        .iter()
        .filter_map(|(enabled, flag)| enabled.then_some(*flag))
        .collect();

    if !ignored.is_empty() {
        ui.warn(format!(
            "{} accepted by zerobrew but not applied yet",
            ignored.join(", ")
        ));
    }
}
