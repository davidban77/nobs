use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use nobs_core::device::{self, Error};

const TEST_PASSWORD: &str = "nobs-t-not-a-secret";

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(relative)
}

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("containerlab")).unwrap();
    dir
}

fn nobs(here: &Path, args: &[&str], user: Option<&str>, password: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command.args(args).current_dir(here).env_remove("NOBS_DIR");
    for (name, value) in [
        ("NETWORK_AGENT_USER", user),
        ("NETWORK_AGENT_PASSWORD", password),
    ] {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    command.output().unwrap()
}

fn with_credentials(here: &Path, args: &[&str]) -> Output {
    nobs(here, args, Some("admin"), Some(TEST_PASSWORD))
}

fn both(name: &str) -> [Vec<&str>; 2] {
    [
        vec!["device", "fetch", name],
        vec!["device", "run", name, "show version"],
    ]
}

fn three(name: &str) -> [Vec<&str>; 3] {
    let [fetch, run] = both(name);
    [fetch, run, vec!["device", "shell", name]]
}

fn four(name: &str) -> [Vec<&str>; 4] {
    let [fetch, run, shell] = three(name);
    [fetch, run, shell, vec!["device", "check"]]
}

fn in_dir<'a>(dir: &'a Path, args: &[&'a str]) -> Vec<&'a str> {
    let mut line = vec!["--dir", dir.to_str().unwrap()];
    line.extend(args);
    line
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_fails_with(output: &Output, line: &str) {
    let stderr = stderr(output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr, format!("{line}\n"));
    assert!(output.stdout.is_empty());
    assert!(!stderr.contains(TEST_PASSWORD));
}

#[test]
fn outside_a_lab_the_device_commands_exit_1_with_the_locate_line() {
    let bare = folder("device-no-lab");
    for args in four("nobs-t-srl1") {
        assert_fails_with(
            &with_credentials(&bare, &args),
            "nobs: no lab or workshop found from here",
        );
        assert_fails_with(
            &with_credentials(&bare, &in_dir(&bare, &args)),
            &format!("nobs: no lab or workshop at {}", bare.display()),
        );
    }
}

#[test]
fn in_a_workshop_the_device_commands_exit_1_with_the_workshop_line() {
    let workshop = fixture("workshop-one");
    for args in four("nobs-t-srl1") {
        assert_fails_with(
            &with_credentials(&workshop, &in_dir(&workshop, &args)),
            &format!(
                "nobs: device commands need a lab with containerlab nodes; {} is a workshop",
                workshop.display()
            ),
        );
    }
}

#[test]
fn a_missing_topology_or_a_text_that_is_not_one_exits_1_with_the_topology_line() {
    let lab = folder("device-topology");
    fs::write(lab.join("nobs.toml"), "kind = \"lab\"\n").unwrap();
    let topology = lab.join("containerlab/lab.clab.yml");
    for args in both("nobs-t-srl1") {
        assert_fails_with(
            &with_credentials(&lab, &in_dir(&lab, &args)),
            &format!("nobs: topology file not found: {}", topology.display()),
        );
    }

    fs::write(&topology, "name: nobs-t-dev\n").unwrap();
    for args in both("nobs-t-srl1") {
        let output = with_credentials(&lab, &in_dir(&lab, &args));
        let stderr = stderr(&output);
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        let line = format!("nobs: cannot read topology file: {}: ", topology.display());
        assert!(stderr.starts_with(&line), "{stderr}");
        assert!(output.stdout.is_empty());
        assert!(!stderr.contains(TEST_PASSWORD));
    }
}

#[test]
fn a_name_the_topology_does_not_hold_exits_1_naming_the_topology_file() {
    let lab = fixture("device-lab");
    let topology = lab.join("containerlab/lab.clab.yml");
    for args in both("nobs-t-srl9") {
        assert_fails_with(
            &with_credentials(&lab, &in_dir(&lab, &args)),
            &format!(
                "nobs: ssh nobs-t-srl9: not a node of {}",
                topology.display()
            ),
        );
    }
}

#[test]
fn a_kind_without_a_driver_exits_1_naming_the_kind() {
    let lab = fixture("device-lab");
    for args in both("nobs-t-ceos1") {
        assert_fails_with(
            &with_credentials(&lab, &in_dir(&lab, &args)),
            "nobs: ssh nobs-t-ceos1: no driver for the kind ceos yet",
        );
    }
}

#[test]
fn a_missing_or_blank_credential_exits_1_naming_both_variables_and_the_places() {
    let lab = fixture("device-lab");
    for (user, password) in [
        (None, Some(TEST_PASSWORD)),
        (Some("admin"), None),
        (Some(""), Some(TEST_PASSWORD)),
        (Some("admin"), Some("  ")),
    ] {
        for args in both("nobs-t-srl1") {
            assert_fails_with(
                &nobs(&lab, &in_dir(&lab, &args), user, password),
                "nobs: ssh nobs-t-srl1: set NETWORK_AGENT_USER and NETWORK_AGENT_PASSWORD in the lab's .env file or in the shell",
            );
        }
    }
}

#[test]
fn a_device_error_is_the_error_line_of_nobs_core() {
    let lab = fixture("device-lab");
    let topology = lab.join("containerlab/lab.clab.yml");
    for (name, error, user) in [
        ("nobs-t-srl9", Error::NotANode(topology), Some("nobs-t-x")),
        (
            "nobs-t-ceos1",
            Error::NoDriver("ceos".to_string()),
            Some("nobs-t-x"),
        ),
        ("nobs-t-srl1", Error::MissingCredentials, None),
    ] {
        assert_fails_with(
            &nobs(
                &lab,
                &in_dir(&lab, &["device", "fetch", name]),
                user,
                Some(TEST_PASSWORD),
            ),
            &device::error_line(name, &error),
        );
    }
}

fn device_lab_with_env(name: &str, env: &str) -> PathBuf {
    let lab = folder(name);
    fs::copy(fixture("device-lab/nobs.toml"), lab.join("nobs.toml")).unwrap();
    fs::copy(
        fixture("device-lab/containerlab/lab.clab.yml"),
        lab.join("containerlab/lab.clab.yml"),
    )
    .unwrap();
    fs::write(lab.join(".env"), env).unwrap();
    lab
}

#[test]
fn a_dot_env_user_of_only_spaces_in_quotes_is_missing_and_opens_no_connection() {
    let lab = device_lab_with_env(
        "device-login-spaces",
        &format!("NETWORK_AGENT_USER=\"  \"\nNETWORK_AGENT_PASSWORD={TEST_PASSWORD}\n"),
    );

    for args in both("nobs-t-srl1") {
        let output = nobs(&lab, &in_dir(&lab, &args), None, None);

        assert_fails_with(
            &output,
            &device::error_line("nobs-t-srl1", &Error::MissingCredentials),
        );
        assert!(!stderr(&output).contains("cannot reach"));
    }
}

#[test]
fn a_shell_user_of_only_spaces_falls_to_the_dot_env_user() {
    let lab = device_lab_with_env(
        "device-login-shell-spaces",
        &format!("NETWORK_AGENT_USER=nobs-t-x\nNETWORK_AGENT_PASSWORD={TEST_PASSWORD}\n"),
    );
    let missing = format!(
        "{}\n",
        device::error_line("nobs-t-srl1", &Error::MissingCredentials)
    );

    for args in both("nobs-t-srl1") {
        let output = nobs(&lab, &in_dir(&lab, &args), Some("   "), None);
        let stderr = stderr(&output);

        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(stderr.starts_with("nobs: ssh nobs-t-srl1: "), "{stderr}");
        assert_ne!(stderr, missing);
        assert!(output.stdout.is_empty());
        assert!(!stderr.contains(TEST_PASSWORD));
    }
}

#[test]
fn the_shell_without_a_terminal_exits_1_with_its_line_and_opens_no_connection() {
    let lab = fixture("device-lab");
    assert_fails_with(
        &with_credentials(&lab, &in_dir(&lab, &["device", "shell", "nobs-t-srl1"])),
        "nobs: the shell needs a terminal",
    );
}

#[test]
fn a_check_of_a_lab_with_no_nodes_exits_0_and_prints_nothing() {
    let lab = folder("device-check-empty");
    fs::write(lab.join("nobs.toml"), "kind = \"lab\"\n").unwrap();
    fs::write(
        lab.join("containerlab/lab.clab.yml"),
        "name: nobs-t-dev\ntopology:\n  nodes: {}\n",
    )
    .unwrap();

    let output = with_credentials(&lab, &in_dir(&lab, &["device", "check"]));

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert_eq!(stderr(&output), "");
}

#[test]
fn check_save_with_no_folder_is_a_usage_error_with_exit_2() {
    let lab = fixture("device-lab");
    let output = with_credentials(&lab, &in_dir(&lab, &["device", "check", "--save"]));
    let stderr = stderr(&output);

    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.starts_with("error: a value is required for '--save <FOLDER>'"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn device_alone_or_a_missing_argument_is_a_usage_error_with_exit_2() {
    let lab = fixture("device-lab");
    for args in [
        &["device"][..],
        &["device", "fetch"][..],
        &["device", "run"][..],
        &["device", "run", "nobs-t-srl1"][..],
        &["device", "shell"][..],
    ] {
        let output = with_credentials(&lab, &in_dir(&lab, args));
        let stderr = stderr(&output);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("Usage: nobs device"), "{args:?}: {stderr}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!stderr.contains(TEST_PASSWORD));
    }
}
