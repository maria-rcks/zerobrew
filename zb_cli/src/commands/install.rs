use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zb_io::{CaskInstallOptions, DownloadProgressCallback, InstallProgress, ProgressCallback};

use crate::ui::{PromptDefault, StdUi};
use crate::utils::{
    PackageKind, normalize_package_name, suggest_homebrew, suggest_missing_formula_matches,
};

pub struct InstallRequest {
    pub formulas: Vec<String>,
    pub no_link: bool,
    pub build_from_source: bool,
    pub ignore_dependencies: bool,
    pub only_dependencies: bool,
    pub ask: bool,
    pub cask: bool,
    pub formula: bool,
    pub appdir: Option<PathBuf>,
    pub fontdir: Option<PathBuf>,
    pub appimagedir: Option<PathBuf>,
    pub no_binaries: bool,
    pub force: bool,
}

pub async fn execute(
    installer: &mut zb_io::Installer,
    request: InstallRequest,
    ui: &mut StdUi,
) -> Result<(), zb_core::Error> {
    let start = Instant::now();
    let formulas = request.formulas;
    ui.heading(format!(
        "Installing {}...",
        style(formulas.join(", ")).bold()
    ))
    .map_err(ui_error)?;

    let mut normalized_names = Vec::new();
    let mut cask_names = Vec::new();
    let kind = if request.cask {
        PackageKind::Cask
    } else if request.formula {
        PackageKind::Formula
    } else {
        PackageKind::Auto
    };
    for formula in &formulas {
        match normalize_package_name(formula, kind) {
            Ok(name) => {
                if name.starts_with("cask:") {
                    cask_names.push(name);
                } else {
                    normalized_names.push(name);
                }
            }
            Err(e) => {
                suggest_homebrew(formula, &e);
                return Err(e);
            }
        }
    }

    let mut installed_count = 0usize;

    if !normalized_names.is_empty() {
        let plan = match installer
            .plan_with_behavior(
                &normalized_names,
                request.build_from_source,
                request.ignore_dependencies,
                request.only_dependencies,
            )
            .await
        {
            Ok(p) => p,
            Err(e) => {
                let handled_missing = suggest_missing_formula_matches(installer, &e).await;

                if !handled_missing {
                    for formula in &formulas {
                        suggest_homebrew(formula, &e);
                    }
                }
                return Err(e);
            }
        };

        ui.heading(format!(
            "Resolving dependencies ({} packages)...",
            plan.items.len()
        ))
        .map_err(ui_error)?;
        for item in &plan.items {
            ui.bullet(format!(
                "{} {}",
                style(&item.formula.name).green(),
                style(&item.formula.versions.stable).dim()
            ))
            .map_err(ui_error)?;
        }

        if request.ask {
            let answer = ui
                .prompt_yes_no("Install these formulae? [y/N]", PromptDefault::No)
                .map_err(ui_error)?;
            if !answer {
                ui.println("Install cancelled.").map_err(ui_error)?;
                return Ok(());
            }
        }

        ui.heading("Downloading and installing formulas...")
            .map_err(ui_error)?;

        let (progress_callback, bars) = create_progress_callback();
        let formula_progress_callback: Arc<ProgressCallback> = Arc::new(Box::new({
            let progress_callback = progress_callback.clone();
            move |event| progress_callback(event)
        }));

        let result_val = installer
            .execute_with_progress(plan, !request.no_link, Some(formula_progress_callback))
            .await;

        finish_progress_bars(&bars);

        let result = match result_val {
            Ok(r) => r,
            Err(ref e @ zb_core::Error::LinkConflict { ref conflicts }) => {
                ui.blank_line().map_err(ui_error)?;
                ui.error("The link step did not complete successfully.")
                    .map_err(ui_error)?;
                ui.println("The formula was installed, but is not symlinked into the prefix.")
                    .map_err(ui_error)?;
                ui.blank_line().map_err(ui_error)?;
                ui.println("Possible conflicting files:")
                    .map_err(ui_error)?;
                for c in conflicts {
                    if let Some(ref owner) = c.owned_by {
                        ui.println(format!(
                            "  {} (symlink belonging to {})",
                            c.path.display(),
                            style(owner).yellow()
                        ))
                        .map_err(ui_error)?;
                    } else {
                        ui.println(format!("  {}", c.path.display()))
                            .map_err(ui_error)?;
                    }
                }
                ui.blank_line().map_err(ui_error)?;
                return Err(e.clone());
            }
            Err(e) => {
                let handled_missing = suggest_missing_formula_matches(installer, &e).await;

                if !handled_missing {
                    for formula in &formulas {
                        suggest_homebrew(formula, &e);
                    }
                }
                return Err(e);
            }
        };
        installed_count += result.installed;
    }

    if !cask_names.is_empty() {
        ui.heading(format!(
            "Installing casks ({} packages)...",
            cask_names.len()
        ))
        .map_err(ui_error)?;
        let mut options = CaskInstallOptions::new(!request.no_link);
        options.binaries = !request.no_binaries;
        options.force = request.force;
        options.app_dir = request.appdir;
        options.font_dir = request.fontdir;
        options.appimage_dir = request.appimagedir;
        let (progress_callback, bars) = create_progress_callback();
        options.progress = Some(progress_callback);
        let result_val = installer
            .install_casks_with_options(&cask_names, options.clone())
            .await;
        finish_progress_bars(&bars);
        let result = match result_val {
            Ok(r) => r,
            Err(ref e @ zb_core::Error::LinkConflict { ref conflicts }) => {
                if request.force {
                    render_cask_link_conflict(ui, conflicts)?;
                    return Err(e.clone());
                }

                let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
                if !interactive {
                    return Err(cask_conflict_error(&cask_names));
                }

                render_cask_link_conflict(ui, conflicts)?;
                if ui
                    .prompt_yes_no(
                        &format!(
                            "Replace existing target and continue? {}",
                            style("[y/N]").dim()
                        ),
                        PromptDefault::No,
                    )
                    .map_err(ui_error)?
                {
                    let mut retry_options = options.clone();
                    retry_options.force = true;
                    let (progress_callback, retry_bars) = create_progress_callback();
                    retry_options.progress = Some(progress_callback);
                    let retry = installer
                        .install_casks_with_options(&cask_names, retry_options)
                        .await;
                    finish_progress_bars(&retry_bars);
                    retry?
                } else {
                    return Err(cask_conflict_error(&cask_names));
                }
            }
            Err(e) => return Err(e),
        };
        installed_count += result.installed;
    }

    let elapsed = start.elapsed();
    ui.blank_line().map_err(ui_error)?;
    ui.heading(format!(
        "Installed {} packages in {:.2}s",
        style(installed_count).green().bold(),
        elapsed.as_secs_f64()
    ))
    .map_err(ui_error)?;

    Ok(())
}

fn ui_error(err: std::io::Error) -> zb_core::Error {
    zb_core::Error::FileError {
        message: format!("failed to write CLI output: {err}"),
    }
}

type ProgressBars = Arc<Mutex<HashMap<String, ProgressBar>>>;

fn create_progress_callback() -> (DownloadProgressCallback, ProgressBars) {
    let multi = MultiProgress::new();
    let bars: ProgressBars = Arc::new(Mutex::new(HashMap::new()));

    let download_style = ProgressStyle::default_bar()
        .template("    {prefix:<16} {bar:25.cyan/dim} {bytes:>10}/{total_bytes:<10} {eta:>6}")
        .unwrap()
        .progress_chars("━━╸");

    let spinner_style = ProgressStyle::default_spinner()
        .template("    {prefix:<16} {spinner:.cyan} {msg}")
        .unwrap()
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");

    let done_style = ProgressStyle::default_spinner()
        .template("    {prefix:<16} {msg}")
        .unwrap();

    let bars_clone = bars.clone();
    let callback: DownloadProgressCallback = Arc::new(move |event| {
        let mut bars = bars_clone.lock().unwrap();
        match event {
            InstallProgress::DownloadStarted { name, total_bytes } => {
                if let Some(pb) = bars.remove(&name) {
                    pb.finish_and_clear();
                }
                let pb = if let Some(total) = total_bytes {
                    let pb = multi.add(ProgressBar::new(total));
                    pb.set_style(download_style.clone());
                    pb
                } else {
                    let pb = multi.add(ProgressBar::new_spinner());
                    pb.set_style(spinner_style.clone());
                    pb.set_message("downloading...");
                    pb.enable_steady_tick(std::time::Duration::from_millis(80));
                    pb
                };
                pb.set_prefix(name.clone());
                bars.insert(name, pb);
            }
            InstallProgress::DownloadProgress {
                name,
                downloaded,
                total_bytes,
            } => {
                if let Some(pb) = bars.get(&name)
                    && total_bytes.is_some()
                {
                    pb.set_position(downloaded);
                }
            }
            InstallProgress::DownloadCompleted { name, total_bytes } => {
                if !bars.contains_key(&name) {
                    let pb = multi.add(ProgressBar::new_spinner());
                    pb.set_style(spinner_style.clone());
                    pb.set_prefix(name.clone());
                    pb.enable_steady_tick(std::time::Duration::from_millis(80));
                    bars.insert(name.clone(), pb);
                }
                if let Some(pb) = bars.get(&name) {
                    if total_bytes > 0 {
                        pb.set_position(total_bytes);
                    }
                    pb.set_style(spinner_style.clone());
                    pb.set_message("unpacking...");
                    pb.enable_steady_tick(std::time::Duration::from_millis(80));
                }
            }
            InstallProgress::UnpackStarted { name } => {
                if !bars.contains_key(&name) {
                    let pb = multi.add(ProgressBar::new_spinner());
                    pb.set_style(spinner_style.clone());
                    pb.set_prefix(name.clone());
                    pb.enable_steady_tick(std::time::Duration::from_millis(80));
                    bars.insert(name.clone(), pb);
                }
                if let Some(pb) = bars.get(&name) {
                    pb.set_message("unpacking...");
                }
            }
            InstallProgress::UnpackCompleted { name } => {
                if let Some(pb) = bars.get(&name) {
                    pb.set_message("unpacked");
                }
            }
            InstallProgress::LinkStarted { name } => {
                if let Some(pb) = bars.get(&name) {
                    pb.set_message("linking...");
                }
            }
            InstallProgress::LinkCompleted { name } => {
                if let Some(pb) = bars.get(&name) {
                    pb.set_message("linked");
                }
            }
            InstallProgress::LinkSkipped { name, reason } => {
                if let Some(pb) = bars.get(&name) {
                    pb.set_message(format!("link skipped ({})", reason));
                }
            }
            InstallProgress::InstallCompleted { name } => {
                if let Some(pb) = bars.get(&name) {
                    pb.set_style(done_style.clone());
                    pb.set_message(format!("{} installed", style("✓").green()));
                    pb.finish();
                }
            }
        }
    });

    (callback, bars)
}

fn finish_progress_bars(bars: &ProgressBars) {
    let bars = bars.lock().unwrap();
    for pb in bars.values() {
        if !pb.is_finished() {
            pb.finish();
        }
    }
}

fn render_cask_link_conflict(
    ui: &mut StdUi,
    conflicts: &[zb_core::ConflictedLink],
) -> Result<(), zb_core::Error> {
    let paths = conflicts
        .iter()
        .map(|c| c.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    ui.warn(format!("Cask target already exists: {paths}"))
        .map_err(ui_error)?;
    Ok(())
}

fn cask_conflict_error(cask_names: &[String]) -> zb_core::Error {
    zb_core::Error::ExecutionError {
        message: format!(
            "cask install stopped; use `{}` to replace existing targets",
            render_cask_retry_command(cask_names)
        ),
    }
}

fn render_cask_retry_command(cask_names: &[String]) -> String {
    let rendered = cask_names
        .iter()
        .map(|name| name.strip_prefix("cask:").unwrap_or(name))
        .collect::<Vec<_>>()
        .join(" ");
    format!("zb install --cask --force {rendered}")
}
