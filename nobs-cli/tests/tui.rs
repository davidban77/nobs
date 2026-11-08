use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const ABOUT: &str = "Plain nobs opens the screen on the lab found from here (or a chooser); the commands below run one step without the screen";

#[test]
fn plain_nobs_without_a_terminal_prints_the_help_on_stderr_and_exits_2() {
    let copy = workshop_copy("tui-plain");
    let docker = recording_docker("tui-plain-docker");

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .current_dir(&copy)
        .env("PATH", path_with(&docker))
        .env_remove("NOBS_DIR")
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "");
    let help = text(&output.stderr);
    assert!(help.starts_with(ABOUT), "{help}");
    assert!(help.contains("Usage: nobs [OPTIONS] [COMMAND]"), "{help}");
    assert!(help.contains("\nCommands:\n"), "{help}");
    assert!(!docker.join("seen").exists(), "a program ran");
}

#[test]
fn plain_nobs_with_dir_and_without_a_terminal_prints_the_help_and_exits_2() {
    let copy = workshop_copy("tui-dir");
    let docker = recording_docker("tui-dir-docker");
    let elsewhere = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .arg("--dir")
        .arg(&copy)
        .current_dir(elsewhere)
        .env("PATH", path_with(&docker))
        .env_remove("NOBS_DIR")
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "");
    let help = text(&output.stderr);
    assert!(help.starts_with(ABOUT), "{help}");
    assert!(help.contains("\nCommands:\n"), "{help}");
    assert!(!docker.join("seen").exists(), "a program ran");
}

#[test]
fn the_root_help_starts_with_the_line_that_says_nobs_opens_the_screen() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .arg("--help")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let help = text(&output.stdout);
    let first = help.lines().next().unwrap_or_default();
    assert_eq!(first, ABOUT, "{help}");
    assert!(first.contains("nobs opens the screen"), "{first}");
}

#[test]
fn the_tui_help_names_the_flags_the_lab_and_the_sonda_server() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["tui", "--help"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let help = text(&output.stdout);
    for part in [
        "--server",
        "--api-key",
        "lab found from here",
        "running sonda-server",
    ] {
        assert!(help.contains(part), "{part}\n{help}");
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn workshop_copy(name: &str) -> PathBuf {
    let copy = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&copy);
    std::fs::create_dir_all(&copy).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/workshop-one");
    for file in ["nobs.toml", "docker-compose.yml"] {
        std::fs::copy(fixture.join(file), copy.join(file)).unwrap();
    }
    copy
}

fn recording_docker(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let _ = std::fs::remove_file(dir.join("seen"));
    let script = dir.join("docker");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{seen}'\n",
            seen = dir.join("seen").display()
        ),
    )
    .unwrap();
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
