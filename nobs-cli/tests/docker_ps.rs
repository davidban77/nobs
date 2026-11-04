use std::ffi::OsString;
use std::path::PathBuf;

use clap::Parser;
use nobs_cli::{run, Cli, Command, DockerCommand};
use nobs_core::runner::FakeRunner;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/one-service/docker-compose.yml")
}

fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(args).unwrap()
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
fn the_compose_file_defaults_to_docker_compose_yml_in_the_current_folder() {
    let cli = parse(&["nobs", "docker", "ps"]);

    let Command::Docker {
        command: DockerCommand::Ps { compose },
    } = cli.command;
    assert_eq!(compose, PathBuf::from("./docker-compose.yml"));
}
