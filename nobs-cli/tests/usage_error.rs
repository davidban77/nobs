use std::process::Command;

#[test]
fn an_unknown_flag_makes_nobs_exit_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--no-such-flag"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
}
