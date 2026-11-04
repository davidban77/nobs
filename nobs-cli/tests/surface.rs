use std::process::Command;

const DOCKER_COMMANDS: [&str; 13] = [
    "build", "pull", "exec", "debug", "start", "stop", "restart", "logs", "ps", "destroy", "rm",
    "update", "network",
];

#[test]
fn the_help_pages_name_the_docker_group_and_exactly_its_thirteen_commands() {
    let commands = commands_block(&help(&[]));
    assert!(commands.iter().any(|word| word == "docker"), "{commands:?}");

    let commands = commands_block(&help(&["docker"]));
    assert_eq!(commands, DOCKER_COMMANDS);
}

#[test]
fn the_rm_help_shows_its_flags_and_never_says_netobs() {
    let help = help(&["docker", "rm"]);
    assert!(help.contains("--volumes"), "{help}");
    assert!(help.contains("--force"), "{help}");
    assert!(!help.contains("netobs"), "{help}");
}

fn help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(args)
        .arg("--help")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{args:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn commands_block(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .filter(|word| *word != "help")
        .map(str::to_owned)
        .collect()
}
