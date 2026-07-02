use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zb_io::{CaskInstallOptions, DownloadProgressCallback, InstallProgress, ProgressCallback};

use crate::ui::{PromptDefault, Ui};
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
    ui: &mut Ui,
) -> Result<(), zb_core::Error> {
    let start = Instant::now();
    let formulas = request.formulas;
    ui.heading(format!(
        "Installing {}...",
        style(formulas.join(", ")).bold()
    ));

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
                suggest_homebrew(ui, formula, &e);
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
                let handled_missing = suggest_missing_formula_matches(installer, &e, ui).await;

                if !handled_missing {
                    for formula in &formulas {
                        suggest_homebrew(ui, formula, &e);
                    }
                }
                return Err(e);
            }
        };

        ui.heading(format!(
            "Resolving dependencies ({} packages)...",
            plan.items.len()
        ));
        for item in &plan.items {
            ui.bullet(format!(
                "{} {}",
                style(&item.formula.name).green(),
                style(&item.formula.versions.stable).dim()
            ));
        }

        if request.ask && !ui.confirm("Install these formulae?", PromptDefault::No) {
            ui.status("Install cancelled.");
            return Ok(());
        }

        ui.heading("Downloading and installing formulas...");

        let renderer = ProgressRenderer::new(ui);
        let progress_callback = renderer.callback();
        let formula_progress_callback: Arc<ProgressCallback> = Arc::new(Box::new({
            let progress_callback = progress_callback.clone();
            move |event| progress_callback(event)
        }));

        let result_val = installer
            .execute_with_progress(plan, !request.no_link, Some(formula_progress_callback))
            .await;

        renderer.finish();

        let result = match result_val {
            Ok(r) => r,
            Err(ref e @ zb_core::Error::LinkConflict { ref conflicts }) => {
                ui.blank_line();
                ui.error("The link step did not complete successfully.");
                ui.status("The formula was installed, but is not symlinked into the prefix.");
                ui.blank_line();
                ui.status("Possible conflicting files:");
                for c in conflicts {
                    if let Some(ref owner) = c.owned_by {
                        ui.status(format!(
                            "  {} (symlink belonging to {})",
                            c.path.display(),
                            style(owner).yellow()
                        ));
                    } else {
                        ui.status(format!("  {}", c.path.display()));
                    }
                }
                ui.blank_line();
                return Err(e.clone());
            }
            Err(e) => {
                let handled_missing = suggest_missing_formula_matches(installer, &e, ui).await;

                if !handled_missing {
                    for formula in &formulas {
                        suggest_homebrew(ui, formula, &e);
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
        ));
        let mut options = CaskInstallOptions::new(!request.no_link);
        options.binaries = !request.no_binaries;
        options.force = request.force;
        options.app_dir = request.appdir;
        options.font_dir = request.fontdir;
        options.appimage_dir = request.appimagedir;
        let renderer = ProgressRenderer::new(ui);
        options.progress = Some(renderer.callback());
        let result_val = installer
            .install_casks_with_options(&cask_names, options.clone())
            .await;
        renderer.finish();
        let result = match result_val {
            Ok(r) => r,
            Err(ref e @ zb_core::Error::LinkConflict { ref conflicts }) => {
                if request.force {
                    render_cask_link_conflict(ui, conflicts);
                    return Err(e.clone());
                }

                if !ui.is_interactive() {
                    return Err(cask_conflict_error(&cask_names));
                }

                render_cask_link_conflict(ui, conflicts);
                if ui.confirm("Replace existing target and continue?", PromptDefault::No) {
                    let mut retry_options = options.clone();
                    retry_options.force = true;
                    let retry_renderer = ProgressRenderer::new(ui);
                    retry_options.progress = Some(retry_renderer.callback());
                    let retry = installer
                        .install_casks_with_options(&cask_names, retry_options)
                        .await;
                    retry_renderer.finish();
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
    ui.blank_line();
    ui.heading(format!(
        "Installed {} packages in {:.2}s",
        style(installed_count).green().bold(),
        elapsed.as_secs_f64()
    ));

    Ok(())
}

/// Renders `InstallProgress` events onto the `Ui`-owned `MultiProgress`.
///
/// Bars draw only when the `Ui` decided progress is enabled (stderr is a TTY
/// and `--quiet` is not set); otherwise events are dropped without any
/// styling or locking work. The bar map lock is never held across indicatif
/// operations: `ProgressBar` is internally reference-counted, so handles are
/// cloned out and terminal redraws happen without serializing the download
/// workers behind the map lock.
pub struct ProgressRenderer {
    multi: MultiProgress,
    bars: Arc<Mutex<HashMap<String, ProgressBar>>>,
    enabled: bool,
}

struct BarStyles {
    download: ProgressStyle,
    spinner: ProgressStyle,
    done: ProgressStyle,
}

impl BarStyles {
    fn new() -> Self {
        Self {
            download: ProgressStyle::default_bar()
                .template(
                    "    {prefix:<16} {bar:25.cyan/dim} {bytes:>10}/{total_bytes:<10} {eta:>6}",
                )
                .expect("static template must parse")
                .progress_chars("━━╸"),
            spinner: ProgressStyle::default_spinner()
                .template("    {prefix:<16} {spinner:.cyan} {msg}")
                .expect("static template must parse")
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
            done: ProgressStyle::default_spinner()
                .template("    {prefix:<16} {msg}")
                .expect("static template must parse"),
        }
    }
}

const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(80);

impl ProgressRenderer {
    pub fn new(ui: &Ui) -> Self {
        Self {
            multi: ui.multi_progress(),
            bars: Arc::new(Mutex::new(HashMap::new())),
            enabled: ui.progress_enabled(),
        }
    }

    pub fn callback(&self) -> DownloadProgressCallback {
        let bars = self.bars.clone();
        let enabled = self.enabled;
        let multi = self.multi.clone();
        let styles = Arc::new(BarStyles::new());

        Arc::new(move |event| {
            if !enabled {
                return;
            }
            render_event(&multi, &bars, &styles, event);
        })
    }

    pub fn finish(&self) {
        if !self.enabled {
            return;
        }
        // Clone the handles out first so the map lock is not held across
        // indicatif operations (which may redraw the terminal).
        let handles: Vec<ProgressBar> = self.bars.lock().unwrap().values().cloned().collect();
        for pb in handles {
            if !pb.is_finished() {
                pb.finish();
            }
        }
    }
}

fn lookup(bars: &Mutex<HashMap<String, ProgressBar>>, name: &str) -> Option<ProgressBar> {
    bars.lock().unwrap().get(name).cloned()
}

fn render_event(
    multi: &MultiProgress,
    bars: &Mutex<HashMap<String, ProgressBar>>,
    styles: &BarStyles,
    event: InstallProgress,
) {
    match event {
        InstallProgress::DownloadStarted { name, total_bytes } => {
            let pb = if let Some(total) = total_bytes {
                let pb = multi.add(ProgressBar::new(total));
                pb.set_style(styles.download.clone());
                pb
            } else {
                let pb = multi.add(ProgressBar::new_spinner());
                pb.set_style(styles.spinner.clone());
                pb.set_message("downloading...");
                pb.enable_steady_tick(TICK_INTERVAL);
                pb
            };
            pb.set_prefix(name.clone());
            let previous = bars.lock().unwrap().insert(name, pb);
            if let Some(previous) = previous {
                previous.finish_and_clear();
            }
        }
        InstallProgress::DownloadProgress {
            name,
            downloaded,
            total_bytes,
        } => {
            if total_bytes.is_some()
                && let Some(pb) = lookup(bars, &name)
            {
                pb.set_position(downloaded);
            }
        }
        InstallProgress::DownloadCompleted { name, total_bytes } => {
            let pb = lookup(bars, &name).unwrap_or_else(|| {
                let pb = multi.add(ProgressBar::new_spinner());
                pb.set_prefix(name.clone());
                bars.lock().unwrap().insert(name.clone(), pb.clone());
                pb
            });
            if total_bytes > 0 {
                pb.set_position(total_bytes);
            }
            pb.set_style(styles.spinner.clone());
            pb.set_message("unpacking...");
            pb.enable_steady_tick(TICK_INTERVAL);
        }
        InstallProgress::UnpackStarted { name } => {
            let pb = lookup(bars, &name).unwrap_or_else(|| {
                let pb = multi.add(ProgressBar::new_spinner());
                pb.set_style(styles.spinner.clone());
                pb.set_prefix(name.clone());
                pb.enable_steady_tick(TICK_INTERVAL);
                bars.lock().unwrap().insert(name.clone(), pb.clone());
                pb
            });
            pb.set_message("unpacking...");
        }
        InstallProgress::UnpackCompleted { name } => {
            if let Some(pb) = lookup(bars, &name) {
                pb.set_message("unpacked");
            }
        }
        InstallProgress::LinkStarted { name } => {
            if let Some(pb) = lookup(bars, &name) {
                pb.set_message("linking...");
            }
        }
        InstallProgress::LinkCompleted { name } => {
            if let Some(pb) = lookup(bars, &name) {
                pb.set_message("linked");
            }
        }
        InstallProgress::LinkSkipped { name, reason } => {
            if let Some(pb) = lookup(bars, &name) {
                pb.set_message(format!("link skipped ({})", reason));
            }
        }
        InstallProgress::InstallCompleted { name } => {
            if let Some(pb) = lookup(bars, &name) {
                pb.set_style(styles.done.clone());
                pb.set_message(format!("{} installed", style("✓").green()));
                pb.finish();
            }
        }
    }
}

fn render_cask_link_conflict(ui: &mut Ui, conflicts: &[zb_core::ConflictedLink]) {
    let paths = conflicts
        .iter()
        .map(|c| c.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    ui.warn(format!("Cask target already exists: {paths}"));
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
