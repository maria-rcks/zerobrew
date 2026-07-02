//! The single arbiter of all terminal output for zerobrew.
//!
//! Stream policy (Unix + Homebrew convention):
//! - **stdout** carries machine-consumable *data* only (`data`, `data_json`):
//!   formula lists, paths, JSON documents. Always safe to pipe.
//! - **stderr** carries all human "chrome": headings, notes, warnings, errors,
//!   step lines, prompts, and progress bars.
//!
//! Color is resolved once per stream at construction (honoring `NO_COLOR`,
//! `CLICOLOR`, `CLICOLOR_FORCE`, `TERM=dumb`, and TTY state) and labels are
//! pre-rendered so no per-line styling allocations happen.
//!
//! Chrome methods are infallible: write errors to a closed/broken stderr are
//! not actionable by commands and are ignored. `SIGPIPE` is reset to its
//! default disposition in `main`, so piping data output into `head` behaves
//! like any other Unix tool instead of surfacing a bogus error.

use console::Style;
use indicatif::{MultiProgress, ProgressDrawTarget};
use serde::Serialize;
use std::fmt::Display;
use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptDefault {
    Yes,
    No,
}

/// How color should be resolved for a stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

impl std::str::FromStr for ColorChoice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            other => Err(format!(
                "invalid color choice '{other}' (expected auto, always, or never)"
            )),
        }
    }
}

/// Output verbosity, wired from the global `--quiet` / `-v` flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiOptions {
    pub quiet: bool,
    pub verbose: u8,
    pub color: ColorChoice,
}

#[derive(Clone)]
pub struct UiStyles {
    pub heading_prefix: Style,
    pub note_label: Style,
    pub info_label: Style,
    pub hint_label: Style,
    pub warn_label: Style,
    pub error_label: Style,
    pub bullet: Style,
    pub step_pending: Style,
    pub step_ok: Style,
    pub step_fail: Style,
}

impl Default for UiStyles {
    fn default() -> Self {
        Self {
            heading_prefix: Style::new().cyan().bold(),
            note_label: Style::new().yellow().bold(),
            info_label: Style::new().cyan().bold(),
            hint_label: Style::new().cyan().bold(),
            warn_label: Style::new().yellow().bold(),
            error_label: Style::new().red().bold(),
            bullet: Style::new(),
            step_pending: Style::new().dim(),
            step_ok: Style::new().green(),
            step_fail: Style::new().red(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiSymbols {
    pub heading_prefix: &'static str,
    pub note_label: &'static str,
    pub info_label: &'static str,
    pub hint_label: &'static str,
    pub warn_label: &'static str,
    pub error_label: &'static str,
    pub bullet: &'static str,
    pub step_pending: &'static str,
    pub step_ok: &'static str,
    pub step_fail: &'static str,
}

impl Default for UiSymbols {
    fn default() -> Self {
        Self {
            heading_prefix: "==>",
            note_label: "Note:",
            info_label: "Info:",
            hint_label: "Hint:",
            warn_label: "Warning:",
            error_label: "error:",
            bullet: "•",
            step_pending: "○",
            step_ok: "✓",
            step_fail: "✗",
        }
    }
}

#[derive(Clone, Default)]
pub struct UiTheme {
    pub styles: UiStyles,
    pub symbols: UiSymbols,
}

/// Labels pre-rendered once (with or without ANSI) so chrome lines cost a
/// single `writeln!` with no styling allocations.
struct Labels {
    heading: String,
    note: String,
    info: String,
    hint: String,
    warn: String,
    error: String,
    bullet: String,
    step_pending: String,
    step_ok: String,
    step_fail: String,
}

impl Labels {
    fn render(theme: &UiTheme, color: bool) -> Self {
        let paint = |style: &Style, symbol: &str| -> String {
            if color {
                style
                    .clone()
                    .force_styling(true)
                    .apply_to(symbol)
                    .to_string()
            } else {
                symbol.to_string()
            }
        };
        let s = &theme.styles;
        let y = &theme.symbols;
        Self {
            heading: paint(&s.heading_prefix, y.heading_prefix),
            note: paint(&s.note_label, y.note_label),
            info: paint(&s.info_label, y.info_label),
            hint: paint(&s.hint_label, y.hint_label),
            warn: paint(&s.warn_label, y.warn_label),
            error: paint(&s.error_label, y.error_label),
            bullet: paint(&s.bullet, y.bullet),
            step_pending: paint(&s.step_pending, y.step_pending),
            step_ok: paint(&s.step_ok, y.step_ok),
            step_fail: paint(&s.step_fail, y.step_fail),
        }
    }
}

/// Shared in-memory writer used by tests to capture a stream.
#[derive(Clone, Default)]
pub struct MemWriter(Arc<Mutex<Vec<u8>>>);

impl MemWriter {
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for MemWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The one output handle every command receives.
pub struct Ui {
    out: Box<dyn Write + Send>,
    err: Box<dyn Write + Send>,
    pub theme: UiTheme,
    labels: Labels,
    quiet: bool,
    verbose: u8,
    color_out: bool,
    color_err: bool,
    interactive: bool,
    progress: MultiProgress,
    progress_enabled: bool,
}

/// Resolve whether a stream should be colored, honoring the conventions
/// documented at <https://no-color.org> and <https://bixense.com/clicolors>.
fn resolve_color(choice: ColorChoice, stream_is_tty: bool) -> bool {
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            if std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| !v.is_empty() && v != "0") {
                return true;
            }
            if std::env::var_os("CLICOLOR").is_some_and(|v| v == "0") {
                return false;
            }
            if std::env::var_os("TERM").is_some_and(|v| v == "dumb") {
                return false;
            }
            stream_is_tty
        }
    }
}

impl Ui {
    /// Construct the process-wide `Ui` from the parsed global flags.
    ///
    /// Also synchronizes the `console` crate's global color switches so that
    /// any residual inline `console::style(...)` call sites follow the same
    /// decision as the `Ui` itself.
    pub fn from_options(options: UiOptions) -> Self {
        let out_is_tty = io::stdout().is_terminal();
        let err_is_tty = io::stderr().is_terminal();
        let color_out = resolve_color(options.color, out_is_tty);
        let color_err = resolve_color(options.color, err_is_tty);
        console::set_colors_enabled(color_out);
        console::set_colors_enabled_stderr(color_err);

        let progress_enabled = err_is_tty && !options.quiet;
        let progress = MultiProgress::with_draw_target(if progress_enabled {
            ProgressDrawTarget::stderr()
        } else {
            ProgressDrawTarget::hidden()
        });

        let theme = UiTheme::default();
        let labels = Labels::render(&theme, color_err);

        // Buffer piped data output (one syscall per ~8 KiB instead of per
        // line); keep TTY output unbuffered so interactive display is live.
        let out: Box<dyn Write + Send> = if out_is_tty {
            Box::new(io::stdout())
        } else {
            Box::new(io::BufWriter::new(io::stdout()))
        };

        Self {
            out,
            err: Box::new(io::stderr()),
            theme,
            labels,
            quiet: options.quiet,
            verbose: options.verbose,
            color_out,
            color_err,
            interactive: io::stdin().is_terminal() && err_is_tty,
            progress,
            progress_enabled,
        }
    }

    /// A `Ui` for tests: captures both streams in memory, no color, no
    /// progress drawing, non-interactive.
    pub fn for_test(options: UiOptions) -> (Self, MemWriter, MemWriter) {
        let out = MemWriter::default();
        let err = MemWriter::default();
        let theme = UiTheme::default();
        let color = matches!(options.color, ColorChoice::Always);
        let labels = Labels::render(&theme, color);
        let ui = Self {
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            theme,
            labels,
            quiet: options.quiet,
            verbose: options.verbose,
            color_out: color,
            color_err: color,
            interactive: false,
            progress: MultiProgress::with_draw_target(ProgressDrawTarget::hidden()),
            progress_enabled: false,
        };
        (ui, out, err)
    }

    /// Re-render cached labels after mutating `theme` (test helper).
    pub fn refresh_theme(&mut self) {
        self.labels = Labels::render(&self.theme, self.color_err);
    }

    // ---------------------------------------------------------------------
    // Introspection
    // ---------------------------------------------------------------------

    pub fn is_quiet(&self) -> bool {
        self.quiet
    }

    pub fn verbose(&self) -> u8 {
        self.verbose
    }

    /// True when we can meaningfully ask the user a question.
    pub fn is_interactive(&self) -> bool {
        self.interactive
    }

    pub fn color_out(&self) -> bool {
        self.color_out
    }

    pub fn color_err(&self) -> bool {
        self.color_err
    }

    /// The shared progress-bar collection. Bars added to it draw on stderr
    /// only when stderr is a TTY and `--quiet` is not set; chrome emitted
    /// through this `Ui` never tears active bars.
    pub fn multi_progress(&self) -> MultiProgress {
        self.progress.clone()
    }

    pub fn progress_enabled(&self) -> bool {
        self.progress_enabled
    }

    // ---------------------------------------------------------------------
    // DATA channel — stdout, never suppressed, always pipe-safe
    // ---------------------------------------------------------------------

    /// Write one line of machine-consumable data to stdout.
    pub fn data(&mut self, line: impl Display) {
        let Self { out, progress, .. } = self;
        let _ = progress.suspend(|| writeln!(out, "{line}"));
    }

    /// Write raw data to stdout without a trailing newline.
    pub fn data_raw(&mut self, chunk: impl Display) {
        let Self { out, progress, .. } = self;
        let _ = progress.suspend(|| write!(out, "{chunk}"));
        let _ = out.flush();
    }

    /// Serialize `value` as pretty JSON followed by a newline on stdout.
    pub fn data_json<T: Serialize>(&mut self, value: &T) -> Result<(), serde_json::Error> {
        let rendered = serde_json::to_string_pretty(value)?;
        self.data(rendered);
        Ok(())
    }

    /// Flush buffered data output. Called before prompts and at exit.
    pub fn flush(&mut self) {
        let _ = self.out.flush();
        let _ = self.err.flush();
    }

    // ---------------------------------------------------------------------
    // CHROME channel — stderr, quiet-gated (warn/error always shown)
    // ---------------------------------------------------------------------

    fn chrome(&mut self, prefix: Option<Chrome>, message: &dyn Display) {
        let label = match prefix {
            Some(Chrome::Heading) => Some((&self.labels.heading, "")),
            Some(Chrome::Note) => Some((&self.labels.note, "")),
            Some(Chrome::Info) => Some((&self.labels.info, "")),
            Some(Chrome::Hint) => Some((&self.labels.hint, "")),
            Some(Chrome::Warn) => Some((&self.labels.warn, "")),
            Some(Chrome::Error) => Some((&self.labels.error, "")),
            Some(Chrome::Bullet) => Some((&self.labels.bullet, "    ")),
            Some(Chrome::Success) => Some((&self.labels.step_ok, "    ")),
            None => None,
        };
        let Self { err, progress, .. } = self;
        let _ = progress.suspend(|| match label {
            Some((label, indent)) => writeln!(err, "{indent}{label} {message}"),
            None => writeln!(err, "{message}"),
        });
    }

    /// `==> message` — a section heading.
    pub fn heading(&mut self, message: impl Display) {
        if !self.quiet {
            self.chrome(Some(Chrome::Heading), &message);
        }
    }

    /// `Note: message`
    pub fn note(&mut self, message: impl Display) {
        if !self.quiet {
            self.chrome(Some(Chrome::Note), &message);
        }
    }

    /// `Info: message`
    pub fn info(&mut self, message: impl Display) {
        if !self.quiet {
            self.chrome(Some(Chrome::Info), &message);
        }
    }

    /// `Hint: message`
    pub fn hint(&mut self, message: impl Display) {
        if !self.quiet {
            self.chrome(Some(Chrome::Hint), &message);
        }
    }

    /// `Warning: message` — always shown, even under `--quiet`.
    pub fn warn(&mut self, message: impl Display) {
        self.chrome(Some(Chrome::Warn), &message);
    }

    /// `error: message` — always shown, even under `--quiet`.
    pub fn error(&mut self, message: impl Display) {
        self.chrome(Some(Chrome::Error), &message);
    }

    /// `    • message` — an indented list item.
    pub fn bullet(&mut self, message: impl Display) {
        if !self.quiet {
            self.chrome(Some(Chrome::Bullet), &message);
        }
    }

    /// A plain chrome line (no prefix) on stderr.
    pub fn status(&mut self, message: impl Display) {
        if !self.quiet {
            self.chrome(None, &message);
        }
    }

    /// `    ✓ message` — a completed-item line.
    pub fn success(&mut self, message: impl Display) {
        if !self.quiet {
            self.chrome(Some(Chrome::Success), &message);
        }
    }

    /// A chrome line shown only with `-v` (or more).
    pub fn detail(&mut self, message: impl Display) {
        if self.verbose > 0 && !self.quiet {
            self.chrome(None, &message);
        }
    }

    /// An empty chrome line.
    pub fn blank_line(&mut self) {
        if !self.quiet {
            self.chrome(None, &"");
        }
    }

    // ---------------------------------------------------------------------
    // Steps
    // ---------------------------------------------------------------------

    /// `    ○ message...` — start of an inline step. Flushed immediately so
    /// the pending state is visible; finish with [`Ui::step_ok`] /
    /// [`Ui::step_fail`].
    pub fn step_start(&mut self, message: impl Display) {
        if self.quiet {
            return;
        }
        let Self {
            err,
            progress,
            labels,
            ..
        } = self;
        let _ = progress.suspend(|| {
            let result = write!(err, "    {} {message}...", labels.step_pending);
            let _ = err.flush();
            result
        });
    }

    pub fn step_ok(&mut self) {
        if self.quiet {
            return;
        }
        let Self {
            err,
            progress,
            labels,
            ..
        } = self;
        let _ = progress.suspend(|| writeln!(err, " {}", labels.step_ok));
    }

    pub fn step_fail(&mut self) {
        if self.quiet {
            return;
        }
        let Self {
            err,
            progress,
            labels,
            ..
        } = self;
        let _ = progress.suspend(|| writeln!(err, " {}", labels.step_fail));
    }

    // ---------------------------------------------------------------------
    // Prompts — written to stderr; never hang when stdin is not a TTY
    // ---------------------------------------------------------------------

    /// Ask a yes/no question. The `[Y/n]` / `[y/N]` suffix is derived from
    /// `default`.
    ///
    /// The prompt is written to stderr and the answer is read from stdin
    /// whether or not stdin is a TTY, so `yes | zb install --ask ...` keeps
    /// working. A closed or empty stdin (CI, `< /dev/null`) hits EOF
    /// immediately and yields the default — prompting can never hang.
    pub fn confirm(&mut self, question: &str, default: PromptDefault) -> bool {
        let stdin_is_tty = io::stdin().is_terminal();
        let answer = {
            let mut stdin = io::stdin().lock();
            self.confirm_with_reader(question, default, &mut stdin)
        };
        if !stdin_is_tty {
            // No terminal echo happened, so finish the prompt line to keep
            // subsequent output cleanly formatted.
            let Self { err, progress, .. } = self;
            let _ = progress.suspend(|| writeln!(err));
        }
        answer
    }

    /// Testable variant of [`Ui::confirm`] with an injected reader.
    pub fn confirm_with_reader<R: BufRead>(
        &mut self,
        question: &str,
        default: PromptDefault,
        reader: &mut R,
    ) -> bool {
        let suffix = match default {
            PromptDefault::Yes => "[Y/n]",
            PromptDefault::No => "[y/N]",
        };
        self.flush();
        {
            let Self { err, progress, .. } = self;
            let _ = progress.suspend(|| {
                let result = write!(err, "{question} {suffix} ");
                let _ = err.flush();
                result
            });
        }

        let mut input = String::new();
        if reader.read_line(&mut input).is_err() {
            return matches!(default, PromptDefault::Yes);
        }
        parse_yes_no_input(&input, default)
    }
}

#[derive(Clone, Copy)]
enum Chrome {
    Heading,
    Note,
    Info,
    Hint,
    Warn,
    Error,
    Bullet,
    Success,
}

fn parse_yes_no_input(input: &str, default: PromptDefault) -> bool {
    let normalized = input.trim().to_ascii_lowercase();

    if normalized.is_empty() {
        return matches!(default, PromptDefault::Yes);
    }

    matches!(normalized.as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::{ColorChoice, PromptDefault, Ui, UiOptions, parse_yes_no_input};
    use std::io::Cursor;

    #[test]
    fn prompt_default_yes_accepts_empty_input() {
        assert!(parse_yes_no_input("", PromptDefault::Yes));
        assert!(parse_yes_no_input("   ", PromptDefault::Yes));
    }

    #[test]
    fn prompt_default_no_rejects_empty_input() {
        assert!(!parse_yes_no_input("", PromptDefault::No));
        assert!(!parse_yes_no_input("   ", PromptDefault::No));
    }

    #[test]
    fn prompt_accepts_yes_tokens_case_insensitively() {
        assert!(parse_yes_no_input("y", PromptDefault::No));
        assert!(parse_yes_no_input("Y", PromptDefault::No));
        assert!(parse_yes_no_input("yes", PromptDefault::No));
        assert!(parse_yes_no_input("YeS", PromptDefault::No));
    }

    #[test]
    fn prompt_rejects_non_yes_tokens() {
        assert!(!parse_yes_no_input("n", PromptDefault::Yes));
        assert!(!parse_yes_no_input("no", PromptDefault::Yes));
        assert!(!parse_yes_no_input("random", PromptDefault::Yes));
    }

    #[test]
    fn confirm_with_reader_uses_default_yes_on_empty_input() {
        let (mut ui, _out, err) = Ui::for_test(UiOptions::default());
        let mut input = Cursor::new("\n");

        let accepted = ui.confirm_with_reader("Continue?", PromptDefault::Yes, &mut input);

        assert!(accepted);
        assert!(err.contents().contains("Continue? [Y/n]"));
    }

    #[test]
    fn confirm_derives_suffix_from_default() {
        let (mut ui, _out, err) = Ui::for_test(UiOptions::default());
        let mut input = Cursor::new("n\n");

        let accepted = ui.confirm_with_reader("Proceed?", PromptDefault::No, &mut input);

        assert!(!accepted);
        assert!(err.contents().contains("Proceed? [y/N]"));
    }

    #[test]
    fn data_goes_to_stdout_and_chrome_to_stderr() {
        let (mut ui, out, err) = Ui::for_test(UiOptions::default());

        ui.heading("Installing jq");
        ui.bullet("jq 1.7");
        ui.warn("something odd");
        ui.data("jq");

        assert_eq!(out.contents(), "jq\n");
        let chrome = err.contents();
        assert!(chrome.contains("==> Installing jq"));
        assert!(chrome.contains("• jq 1.7"));
        assert!(chrome.contains("Warning: something odd"));
        assert!(!chrome.contains("jq\n==>"));
    }

    #[test]
    fn quiet_suppresses_chrome_but_not_data_or_errors() {
        let (mut ui, out, err) = Ui::for_test(UiOptions {
            quiet: true,
            ..Default::default()
        });

        ui.heading("Installing jq");
        ui.note("a note");
        ui.status("status line");
        ui.data("jq");
        ui.warn("kept warning");
        ui.error("kept error");

        assert_eq!(out.contents(), "jq\n");
        let chrome = err.contents();
        assert!(!chrome.contains("==>"));
        assert!(!chrome.contains("a note"));
        assert!(!chrome.contains("status line"));
        assert!(chrome.contains("Warning: kept warning"));
        assert!(chrome.contains("error: kept error"));
    }

    #[test]
    fn detail_requires_verbose() {
        let (mut ui, _out, err) = Ui::for_test(UiOptions::default());
        ui.detail("hidden");
        assert!(!err.contents().contains("hidden"));

        let (mut ui, _out, err) = Ui::for_test(UiOptions {
            verbose: 1,
            ..Default::default()
        });
        ui.detail("shown");
        assert!(err.contents().contains("shown"));
    }

    #[test]
    fn no_color_by_default_in_tests() {
        let (mut ui, _out, err) = Ui::for_test(UiOptions::default());
        ui.heading("hello");
        assert!(!err.contents().contains('\u{1b}'));
    }

    #[test]
    fn forced_color_renders_ansi() {
        let (mut ui, _out, err) = Ui::for_test(UiOptions {
            color: ColorChoice::Always,
            ..Default::default()
        });
        ui.heading("hello");
        assert!(err.contents().contains('\u{1b}'));
    }

    #[test]
    fn heading_respects_theme_symbols() {
        let (mut ui, _out, err) = Ui::for_test(UiOptions::default());
        ui.theme.symbols.heading_prefix = "->";
        ui.refresh_theme();

        ui.heading("hello");

        let stripped = console::strip_ansi_codes(&err.contents()).into_owned();
        assert!(stripped.contains("-> hello"));
    }

    #[test]
    fn success_renders_indented_checkmark_on_stderr() {
        let (mut ui, out, err) = Ui::for_test(UiOptions::default());
        ui.success("Removed abc123");
        assert_eq!(err.contents(), "    ✓ Removed abc123\n");
        assert!(out.contents().is_empty());
    }

    #[test]
    fn step_start_is_flushed_and_completed_inline() {
        let (mut ui, _out, err) = Ui::for_test(UiOptions::default());
        ui.step_start("working");
        assert!(err.contents().contains("○ working..."));
        ui.step_ok();
        assert!(err.contents().contains("○ working... ✓\n"));
    }

    #[test]
    fn data_json_serializes_to_stdout() {
        let (mut ui, out, err) = Ui::for_test(UiOptions::default());
        ui.data_json(&vec!["jq", "wget"]).unwrap();
        assert!(out.contents().contains("\"jq\""));
        assert!(err.contents().is_empty());
    }
}
