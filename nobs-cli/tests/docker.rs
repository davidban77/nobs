use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::PathBuf;

use clap::Parser;
use nobs_cli::{run, Cli};
use nobs_core::runner::FakeRunner;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/one-service/docker-compose.yml")
}

fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(args).unwrap()
}

fn words(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[test]
fn the_child_exit_code_is_the_exit_code_of_nobs() {
    let file = fixture();
    let runner = FakeRunner::returning(3);
    let cli = parse(&["nobs", "docker", "ps", "--compose", file.to_str().unwrap()]);

    assert_eq!(run(cli, &runner), 3);

    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "docker");
    assert_eq!(
        calls[0].args,
        [
            OsString::from("compose"),
            OsString::from("-f"),
            file.into_os_string(),
            OsString::from("ps"),
            OsString::from("--all"),
        ]
    );
}

#[test]
fn a_missing_compose_file_exits_1_and_runs_nothing() {
    let runner = FakeRunner::returning(0);
    let cli = parse(&[
        "nobs",
        "docker",
        "ps",
        "--compose",
        "tests/fixtures/missing.yml",
    ]);

    assert_eq!(run(cli, &runner), 1);
    assert!(runner.calls().is_empty());
}

#[test]
fn a_runner_that_cannot_start_docker_makes_run_return_1() {
    let runner = FakeRunner::failing();
    let cli = parse(&[
        "nobs",
        "docker",
        "ps",
        "--compose",
        fixture().to_str().unwrap(),
    ]);

    assert_eq!(run(cli, &runner), 1);
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn every_command_returns_the_child_exit_code_after_one_call() {
    let file = fixture();
    let file = file.to_str().unwrap();
    let cases: [(Vec<&str>, &str); 13] = [
        (vec!["build", "--compose", file], "compose"),
        (vec!["pull", "--compose", file], "compose"),
        (
            vec!["exec", "--compose", file, "sleeper", "true"],
            "compose",
        ),
        (vec!["debug", "--compose", file], "compose"),
        (vec!["start", "--compose", file], "compose"),
        (vec!["stop", "--compose", file], "compose"),
        (vec!["restart", "--compose", file], "compose"),
        (vec!["logs", "--compose", file], "compose"),
        (vec!["ps", "--compose", file], "compose"),
        (vec!["destroy", "--compose", file], "compose"),
        (vec!["rm", "--compose", file], "compose"),
        (vec!["update", "--compose", file], "compose"),
        (vec!["network", "ls"], "network"),
    ];

    for (command, first) in cases {
        let runner = FakeRunner::returning(3);
        let mut args = vec!["nobs", "docker"];
        args.extend(command);

        assert_eq!(run(parse(&args), &runner), 3, "{args:?}");

        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "{args:?}");
        assert_eq!(calls[0].program, "docker", "{args:?}");
        assert_eq!(calls[0].args[0], first, "{args:?}");
    }
}

#[test]
fn the_flags_and_services_typed_on_the_command_line_reach_the_list() {
    let file = fixture();
    let file = file.to_str().unwrap();
    let rows = [
        (
            vec![
                "build",
                "--compose",
                file,
                "-p",
                "a",
                "-p",
                "b",
                "--verbose",
            ],
            Some((
                1,
                vec![
                    "-f",
                    file,
                    "--profile",
                    "a",
                    "--profile",
                    "b",
                    "--verbose",
                    "build",
                ],
            )),
        ),
        (
            vec!["logs", "--compose", file, "-f", "-t", "5", "sleeper"],
            Some((1, vec!["logs", "-f", "--tail=5", "sleeper"])),
        ),
        (
            vec!["destroy", "--compose", file, "--volumes"],
            Some((1, vec!["down", "--volumes", "--remove-orphans"])),
        ),
        (
            vec!["rm", "--compose", file, "--volumes", "--force"],
            Some((1, vec!["rm", "--stop", "--force", "--volumes"])),
        ),
        (
            vec!["update", "--compose", file, "--volumes", "sleeper"],
            Some((
                2,
                vec!["--force-recreate", "--renew-anon-volumes", "sleeper"],
            )),
        ),
        (
            vec!["stop", "--compose", file, "a", "b"],
            Some((1, vec!["stop", "a", "b"])),
        ),
        (
            vec!["exec", "--compose", file, "sleeper", "--", "--flag"],
            Some((1, vec!["sleeper", "--flag"])),
        ),
        (vec!["network", "bogus"], None),
    ];

    for (typed, expected) in rows {
        let mut args = vec!["nobs", "docker"];
        args.extend(typed);
        let parsed = Cli::try_parse_from(&args);
        let Some((calls, tail)) = expected else {
            assert!(parsed.is_err(), "{args:?}");
            continue;
        };
        let cli = parsed.unwrap_or_else(|error| panic!("{args:?}: {error}"));
        let runner = FakeRunner::returning(0);

        run(cli, &runner);

        let recorded = runner.calls();
        assert_eq!(recorded.len(), calls, "{args:?}");
        let last = &recorded[calls - 1].args;
        assert!(last.ends_with(&words(&tail)), "{args:?} gave {last:?}");
    }
}

#[test]
fn a_scenario_whose_file_is_missing_returns_1_and_runs_nothing() {
    let runner = FakeRunner::returning(0);
    let cli = parse(&["nobs", "docker", "ps", "--scenario", "ch5"]);

    assert_eq!(run(cli, &runner), 1);
    assert!(runner.calls().is_empty());
}

#[test]
fn compose_and_scenario_together_do_not_parse() {
    let result = Cli::try_parse_from([
        "nobs",
        "docker",
        "ps",
        "--compose",
        "lab/docker-compose.yml",
        "--scenario",
        "ch5",
    ]);

    assert!(result.is_err());
}

#[test]
fn exec_defaults_the_command_to_bash_and_drops_the_terminal_when_stdin_is_none() {
    let file = fixture();
    let file = file.to_str().unwrap();
    let runner = FakeRunner::returning(0);
    run(
        parse(&["nobs", "docker", "exec", "--compose", file, "sleeper"]),
        &runner,
    );
    let args = runner.calls()[0].args.clone();
    assert!(args.ends_with(&words(&["sleeper", "bash"])), "{args:?}");
    let no_terminal = !std::io::stdin().is_terminal();
    assert_eq!(
        args.contains(&OsString::from("-T")),
        no_terminal,
        "{args:?}"
    );

    let runner = FakeRunner::returning(0);
    run(
        parse(&[
            "nobs",
            "docker",
            "exec",
            "--compose",
            file,
            "sleeper",
            "true",
        ]),
        &runner,
    );
    let args = runner.calls()[0].args.clone();
    assert!(args.ends_with(&words(&["sleeper", "true"])), "{args:?}");
}

#[test]
fn update_pulls_then_recreates_and_stops_after_a_failed_pull() {
    let file = fixture();
    let file = file.to_str().unwrap();
    let runner = FakeRunner::returning(0);
    let cli = parse(&["nobs", "docker", "update", "--compose", file, "sleeper"]);

    assert_eq!(run(cli, &runner), 0);

    let calls = runner.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].args[3], "pull");
    assert_eq!(calls[1].args[3], "up");
    assert!(calls[0].args.ends_with(&words(&["sleeper"])), "{calls:?}");
    assert!(calls[1].args.ends_with(&words(&["sleeper"])), "{calls:?}");

    let runner = FakeRunner::returning(3);
    let cli = parse(&["nobs", "docker", "update", "--compose", file, "sleeper"]);

    assert_eq!(run(cli, &runner), 3);
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn network_connect_needs_a_name_and_network_ls_does_not() {
    let runner = FakeRunner::returning(0);
    let cli = parse(&["nobs", "docker", "network", "connect"]);

    assert_eq!(run(cli, &runner), 2);
    assert!(runner.calls().is_empty());

    let runner = FakeRunner::returning(0);
    let cli = parse(&["nobs", "docker", "network", "ls"]);

    assert_eq!(run(cli, &runner), 0);
    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args, ["network", "ls"]);
}

#[test]
fn docker_compose_with_hash_changes_nothing() {
    std::env::set_var("DOCKER_COMPOSE_WITH_HASH", "1");
    let runner = FakeRunner::returning(0);
    let cli = parse(&[
        "nobs",
        "docker",
        "ps",
        "--compose",
        fixture().to_str().unwrap(),
    ]);

    run(cli, &runner);

    let calls = runner.calls();
    assert_eq!(calls[0].program, "docker");
    assert_eq!(calls[0].args[0], "compose");
}
