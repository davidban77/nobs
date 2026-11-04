use std::process::Command;

#[test]
fn an_unknown_flag_makes_nobs_exit_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--no-such-flag"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn compose_and_scenario_together_make_nobs_exit_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args([
            "docker",
            "ps",
            "--compose",
            "lab/docker-compose.yml",
            "--scenario",
            "ch5",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn exec_without_a_service_makes_nobs_exit_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args([
            "docker",
            "exec",
            "--compose",
            "tests/fixtures/labcli-example/docker-compose.yml",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
}
