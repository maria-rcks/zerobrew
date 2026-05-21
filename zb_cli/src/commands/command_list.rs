use console::style;

const BUILT_IN_COMMANDS: &[&str] = &[
    "bundle",
    "commands",
    "completion",
    "doctor",
    "gc",
    "info",
    "init",
    "install",
    "link",
    "list",
    "migrate",
    "outdated",
    "reset",
    "run",
    "shellenv",
    "uninstall",
    "unlink",
    "update",
];

const COMMAND_ALIASES: &[&str] = &[];

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
}
