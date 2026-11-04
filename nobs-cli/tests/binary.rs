use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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

#[test]
fn the_compose_file_defaults_to_docker_compose_yml_in_the_current_folder() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps"])
        .current_dir(empty_folder())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("./docker-compose.yml"), "{stderr}");
}

#[test]
fn a_scenario_names_its_compose_file_under_chapters() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--scenario", "ch5"])
        .current_dir(empty_folder())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("./chapters/ch5/docker-compose.yml"),
        "{stderr}"
    );
}

#[test]
fn every_command_exits_with_the_code_of_the_docker_program_on_path() {
    let dir = fake_docker("passthrough");
    let path = path_with(&dir);
    let fixture = fixture();
    let file = fixture.to_str().unwrap();
    let commands: [Vec<&str>; 13] = [
        vec!["build", "--compose", file],
        vec!["pull", "--compose", file],
        vec!["exec", "--compose", file, "sleeper", "true"],
        vec!["debug", "--compose", file],
        vec!["start", "--compose", file],
        vec!["stop", "--compose", file],
        vec!["restart", "--compose", file],
        vec!["logs", "--compose", file],
        vec!["ps", "--compose", file],
        vec!["destroy", "--compose", file],
        vec!["rm", "--compose", file],
        vec!["update", "--compose", file],
        vec!["network", "ls"],
    ];

    for command in commands {
        let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
            .arg("docker")
            .args(&command)
            .env("PATH", &path)
            .env("NOBS_T_CODE", "5")
            .output()
            .unwrap();

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(5), "{command:?}: {stderr}");
    }
}

#[test]
fn a_value_from_dot_env_reaches_the_docker_child() {
    let dir = fake_docker("dot-env");
    std::fs::write(dir.join(".env"), "NOBS_T_CODE=5\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--compose"])
        .arg(fixture())
        .current_dir(&dir)
        .env("PATH", path_with(&dir))
        .env_remove("NOBS_T_CODE")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(5), "{stderr}");
}

#[test]
fn an_empty_key_in_dot_env_exits_1_before_docker_starts() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cli-empty-key");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".env"), "=x\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--compose"])
        .arg(fixture())
        .current_dir(&dir)
        .env("PATH", empty_folder())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("empty key"), "{stderr}");
    assert!(!stderr.contains("cannot run docker"), "{stderr}");
}

fn empty_folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("empty-path");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fake_docker(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("docker");
    std::fs::write(&script, "#!/bin/sh\nexit \"${NOBS_T_CODE:-9}\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn path_with(dir: &Path) -> OsString {
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}
