use crate::ui::Ui;

const BUILT_IN_COMMANDS: &[&str] = &[
    "autoremove",
    "bundle",
    "casks",
    "cat",
    "cellar",
    "cleanup",
    "commands",
    "completion",
    "config",
    "deps",
    "doctor",
    "edit",
    "formulae",
    "gc",
    "home",
    "info",
    "init",
    "install",
    "leaves",
    "link",
    "list",
    "migrate",
    "missing",
    "options",
    "outdated",
    "prefix",
    "reinstall",
    "reset",
    "run",
    "search",
    "shellenv",
    "uninstall",
    "unlink",
    "update",
    "upgrade",
    "uses",
];

const COMMAND_ALIASES: &[&str] = &[
    "--cellar", "--prefix", "add", "b", "cfg", "check", "clean", "cmds", "desc", "env", "find",
    "homepage", "i", "leaf", "ln", "ls", "old", "prune", "re", "remove", "rm", "show", "ug",
    "unln", "up",
];

/// `quiet` is the subcommand-local `--quiet` flag (plain names for shell
/// completion), distinct from the global `--quiet` carried by `ui`.
pub fn execute(quiet: bool, include_aliases: bool, ui: &mut Ui) -> Result<(), zb_core::Error> {
    let mut commands = BUILT_IN_COMMANDS.to_vec();
    if include_aliases {
        commands.extend_from_slice(COMMAND_ALIASES);
        commands.sort_unstable();
    }

    if quiet {
        for command in commands {
            ui.data(command);
        }
    } else {
        ui.heading("Built-in commands");
        ui.data(commands.join(" "));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BUILT_IN_COMMANDS, execute};
    use crate::ui::{Ui, UiOptions};

    #[test]
    fn command_list_stays_sorted() {
        let mut sorted = BUILT_IN_COMMANDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(BUILT_IN_COMMANDS, sorted);
    }

    #[test]
    fn command_aliases_stay_sorted() {
        let mut sorted = super::COMMAND_ALIASES.to_vec();
        sorted.sort_unstable();
        assert_eq!(super::COMMAND_ALIASES, sorted);
    }

    #[test]
    fn local_quiet_emits_plain_names_on_stdout_only() {
        let (mut ui, out, err) = Ui::for_test(UiOptions::default());

        execute(true, false, &mut ui).unwrap();

        let stdout = out.contents();
        assert!(stdout.lines().any(|line| line == "install"));
        assert_eq!(stdout.lines().count(), BUILT_IN_COMMANDS.len());
        assert!(err.contents().is_empty());
    }

    #[test]
    fn default_output_splits_heading_and_data() {
        let (mut ui, out, err) = Ui::for_test(UiOptions::default());

        execute(false, true, &mut ui).unwrap();

        // Heading is chrome (stderr); the command list itself is data (stdout).
        assert!(err.contents().contains("==> Built-in commands"));
        let stdout = out.contents();
        assert!(stdout.contains("install"));
        assert!(stdout.contains("cmds"));
        assert!(!stdout.contains("==>"));
    }
}
