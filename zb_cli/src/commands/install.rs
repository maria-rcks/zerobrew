use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zb_io::{CaskInstallOptions, InstallProgress, ProgressCallback};

use crate::ui::StdUi;
use crate::utils::{
    PackageKind, normalize_package_name, suggest_homebrew, suggest_missing_formula_matches,
};

pub struct InstallRequest {
    pub formulas: Vec<String>,
    pub no_link: bool,
    pub build_from_source: bool,
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
            .plan_with_options(&normalized_names, request.build_from_source)
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

        let multi = MultiProgress::new();
        let bars: Arc<Mutex<HashMap<String, ProgressBar>>> = Arc::new(Mutex::new(HashMap::new()));

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

        ui.heading("Downloading and installing formulas...")
            .map_err(ui_error)?;

        let bars_clone = bars.clone();
        let multi_clone = multi.clone();
        let download_style_clone = download_style.clone();
        let spinner_style_clone = spinner_style.clone();
        let done_style_clone = done_style.clone();

        let progress_callback: Arc<ProgressCallback> = Arc::new(Box::new(move |event| {
            let mut bars = bars_clone.lock().unwrap();
            match event {
                InstallProgress::DownloadStarted { name, total_bytes } => {
                    let pb = if let Some(total) = total_bytes {
                        let pb = multi_clone.add(ProgressBar::new(total));
                        pb.set_style(download_style_clone.clone());
                        pb
                    } else {
                        let pb = multi_clone.add(ProgressBar::new_spinner());
                        pb.set_style(spinner_style_clone.clone());
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
                    if let Some(pb) = bars.get(&name) {
                        if total_bytes > 0 {
                            pb.set_position(total_bytes);
                        }
                        pb.set_style(spinner_style_clone.clone());
                        pb.set_message("unpacking...");
                        pb.enable_steady_tick(std::time::Duration::from_millis(80));
                    }
                }
                InstallProgress::UnpackStarted { name } => {
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
                        pb.set_message(format!("keg-only ({})", reason));
                    }
                }
                InstallProgress::InstallCompleted { name } => {
                    if let Some(pb) = bars.get(&name) {
                        pb.set_style(done_style_clone.clone());
                        pb.set_message(format!("{} installed", style("✓").green()));
                        pb.finish();
                    }
                }
            }
        }));

        let result_val = installer
            .execute_with_progress(plan, !request.no_link, Some(progress_callback))
            .await;

        {
            let bars = bars.lock().unwrap();
            for (_, pb) in bars.iter() {
                if !pb.is_finished() {
                    pb.finish();
                }
            }
        }

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
        let result = installer
            .install_casks_with_options(&cask_names, options)
            .await?;
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
