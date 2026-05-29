use console::style;

const BUILT_IN_COMMANDS: &[&str] = &[
    "autoremove",
    "bundle",
    "cleanup",
    "commands",
    "completion",
    "config",
    "deps",
    "doctor",
    "gc",
    "info",
    "init",
    "install",
    "leaves",
    "link",
    "list",
    "migrate",
    "missing",
    "outdated",
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
    "add", "b", "cat", "cfg", "check", "clean", "cmds", "desc", "env", "find", "home", "homepage",
    "i", "leaf", "ln", "ls", "old", "prune", "re", "remove", "rm", "show", "ug", "unln", "up",
];

pub fn execute(quiet: bool, include_aliases: bool) -> Result<(), zb_core::Error> {
    let mut commands = BUILT_IN_COMMANDS.to_vec();
    if include_aliases {
        commands.extend_from_slice(COMMAND_ALIASES);
        commands.sort_unstable();
    }

    if quiet {
        for command in commands {
            println!("{command}");
        }
    } else {
        println!(
            "{}\n{}",
            style("Built-in commands").cyan().bold(),
            commands.join(" ")
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::BUILT_IN_COMMANDS;

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
}
