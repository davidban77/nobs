use std::ffi::OsString;
use std::path::PathBuf;

use clap::Parser;
use nobs_cli::{run, Cli};
use nobs_core::runner::FakeRunner;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/srlinux-two-node/lab.clab.yml")
}

fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(args).unwrap()
}

fn words(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[test]
fn deploy_runs_sudo_with_the_variables_of_the_topology_kept() {
    let file = fixture();
    let file = file.to_str().unwrap();
    let runner = FakeRunner::returning(0);

    assert_eq!(
        run(parse(&["nobs", "containerlab", "deploy", file]), &runner),
        0
    );

    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "sudo");
    assert_eq!(
        calls[0].args,
        words(&[
            "--preserve-env=NOBS_T_LABEL",
            "containerlab",
            "deploy",
            "-t",
            file
        ])
    );
}

#[test]
fn no_sudo_runs_containerlab_itself() {
    let file = fixture();
    let file = file.to_str().unwrap();
    let runner = FakeRunner::returning(0);
    let cli = parse(&["nobs", "containerlab", "deploy", "--no-sudo", file]);

    assert_eq!(run(cli, &runner), 0);

    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "containerlab");
    assert_eq!(calls[0].args, words(&["deploy", "-t", file]));
}

#[test]
fn the_last_of_sudo_and_no_sudo_wins() {
    let file = fixture();
    let file = file.to_str().unwrap();
    let cases = [
        (["--sudo", "--no-sudo"], "containerlab"),
        (["--no-sudo", "--sudo"], "sudo"),
    ];

    for (flags, program) in cases {
        let runner = FakeRunner::returning(0);
        let cli = parse(&["nobs", "containerlab", "inspect", flags[0], flags[1], file]);

        assert_eq!(run(cli, &runner), 0);
        assert_eq!(runner.calls()[0].program, program, "{flags:?}");
    }
}

#[test]
fn every_command_returns_the_child_exit_code_after_one_call() {
    let file = fixture();
    let file = file.to_str().unwrap();

    for command in ["deploy", "destroy", "inspect"] {
        let runner = FakeRunner::returning(3);
        let cli = parse(&["nobs", "containerlab", command, file]);

        assert_eq!(run(cli, &runner), 3, "{command}");
        assert_eq!(runner.calls().len(), 1, "{command}");
    }
}

#[test]
fn a_missing_topology_exits_1_and_runs_nothing() {
    let runner = FakeRunner::returning(0);
    let cli = parse(&[
        "nobs",
        "containerlab",
        "deploy",
        "tests/fixtures/missing.yml",
    ]);

    assert_eq!(run(cli, &runner), 1);
    assert!(runner.calls().is_empty());
}

#[test]
fn a_topology_that_is_not_text_exits_1_and_runs_nothing() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("topology-not-text");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("lab.clab.yml");
    std::fs::write(&file, [0xff, 0xfe, 0xfd]).unwrap();
    let runner = FakeRunner::returning(0);
    let cli = parse(&["nobs", "containerlab", "deploy", file.to_str().unwrap()]);

    assert_eq!(run(cli, &runner), 1);
    assert!(runner.calls().is_empty());
}

#[test]
fn a_runner_that_cannot_start_the_program_makes_run_return_1() {
    let runner = FakeRunner::failing();
    let cli = parse(&[
        "nobs",
        "containerlab",
        "deploy",
        fixture().to_str().unwrap(),
    ]);

    assert_eq!(run(cli, &runner), 1);
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn the_fixture_is_a_two_node_srlinux_lab() {
    let text = std::fs::read_to_string(fixture()).unwrap();

    for part in [
        "name: nobs-t-lab",
        "nobs-t-srl1",
        "nobs-t-srl2",
        "network: nobs-t-net",
        "${NOBS_T_LABEL",
        "ghcr.io/nokia/srlinux:latest",
    ] {
        assert!(text.contains(part), "{part}");
    }
}
