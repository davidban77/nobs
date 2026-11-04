use std::path::PathBuf;
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/one-service/docker-compose.yml")
}

#[test]
fn a_missing_compose_file_exits_1_and_names_the_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--compose", "missing.yml"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("compose file not found"), "{stderr}");
    assert!(stderr.contains("missing.yml"), "{stderr}");
}

#[test]
fn nobs_exits_1_and_says_so_when_docker_cannot_start() {
    let empty = empty_folder();
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--compose"])
        .arg(fixture())
        .env("PATH", &empty)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot run docker"), "{stderr}");
}

fn empty_folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("empty-path");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
