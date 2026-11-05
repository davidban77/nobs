use std::process::Command;

const DOCKER_COMMANDS: [&str; 13] = [
    "build", "pull", "exec", "debug", "start", "stop", "restart", "logs", "ps", "destroy", "rm",
    "update", "network",
];

const CONTAINERLAB_COMMANDS: [&str; 3] = ["deploy", "destroy", "inspect"];

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

#[test]
fn the_help_pages_name_the_containerlab_group_and_exactly_its_three_commands() {
    let commands = commands_block(&help(&[]));
    assert!(
        commands.iter().any(|word| word == "containerlab"),
        "{commands:?}"
    );

    let commands = commands_block(&help(&["containerlab"]));
    assert_eq!(commands, CONTAINERLAB_COMMANDS);
}

const SONDA_COMMANDS: [&str; 10] = [
    "health", "ready", "list", "show", "stats", "delete", "load", "event", "metrics", "version",
];

#[test]
fn the_help_pages_name_the_sonda_group_and_exactly_its_ten_commands() {
    let commands = commands_block(&help(&[]));
    for group in ["docker", "containerlab", "sonda"] {
        assert!(commands.iter().any(|word| word == group), "{commands:?}");
    }

    let commands = commands_block(&help(&["sonda"]));
    assert_eq!(commands, SONDA_COMMANDS);
}

#[test]
fn a_sonda_command_help_shows_the_server_and_key_flags() {
    let help = help(&["sonda", "list"]);
    assert!(help.contains("--server"), "{help}");
    assert!(help.contains("--api-key"), "{help}");
}

#[test]
fn the_destroy_help_never_says_deploy() {
    let help = help(&["containerlab", "destroy"]);
    assert!(!help.contains("Deploy"), "{help}");
}

#[test]
fn no_containerlab_help_page_says_show_netobs_or_labcli() {
    let pages: [&[&str]; 5] = [
        &[],
        &["containerlab"],
        &["containerlab", "deploy"],
        &["containerlab", "destroy"],
        &["containerlab", "inspect"],
    ];

    for page in pages {
        let help = help(page);
        for word in ["show", "netobs", "labcli"] {
            assert!(!help.contains(word), "{page:?}: {word}\n{help}");
        }
    }
}

#[test]
fn the_root_help_names_the_groups_workshops_and_the_dir_flag() {
    let help = help(&[]);
    let commands = commands_block(&help);
    for command in ["docker", "containerlab", "sonda", "workshops"] {
        assert!(commands.iter().any(|word| word == command), "{commands:?}");
    }
    assert!(help.contains("--dir <PATH>"), "{help}");
}

#[test]
fn the_root_help_names_setup_and_preflight_with_their_descriptions() {
    let help = help(&[]);
    let commands = commands_block(&help);
    for command in ["setup", "preflight", "workshops"] {
        assert!(commands.iter().any(|word| word == command), "{commands:?}");
    }
    assert!(
        help.contains("Check that this machine can run a workshop"),
        "{help}"
    );
    assert!(
        help.contains(
            "Check this machine, then create the .env file of every workshop nobs can find"
        ),
        "{help}"
    );
}

#[test]
fn the_workshop_help_page_inside_a_workshop_names_exactly_the_ten_commands_in_order() {
    let help = workshop_help(&["nobs-t-one", "--help"]);
    assert_eq!(
        commands_block(&help),
        ["setup", "up", "down", "destroy", "restart", "ps", "logs", "exec", "build", "status"]
    );
}

#[test]
fn the_workshop_status_help_shows_the_seven_address_flags() {
    let help = workshop_help(&["nobs-t-one", "status", "--help"]);
    for flag in [
        "--prom-url",
        "--loki-url",
        "--am-url",
        "--infrahub-url",
        "--grafana-url",
        "--prefect-url",
        "--sonda-url",
    ] {
        let option = format!("{flag} <URL>");
        assert!(
            help.lines()
                .any(|line| line.trim_start().starts_with(&option)),
            "{flag}\n{help}"
        );
    }
}

fn workshop_help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(args)
        .current_dir(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/workshop-one"),
        )
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{args:?}");
    String::from_utf8(output.stdout).unwrap()
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
