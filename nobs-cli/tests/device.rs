use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
    for args in both("nobs-t-srl1") {
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
    for args in both("nobs-t-srl1") {
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
fn device_alone_or_a_missing_argument_is_a_usage_error_with_exit_2() {
    let lab = fixture("device-lab");
    for args in [
        &["device"][..],
        &["device", "fetch"][..],
        &["device", "run"][..],
        &["device", "run", "nobs-t-srl1"][..],
    ] {
        let output = with_credentials(&lab, &in_dir(&lab, args));
        let stderr = stderr(&output);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("Usage: nobs device"), "{args:?}: {stderr}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!stderr.contains(TEST_PASSWORD));
    }
}
