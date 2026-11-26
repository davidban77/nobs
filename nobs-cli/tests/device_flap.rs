#[path = "support/device_lab.rs"]
mod device_lab;

use std::path::Path;
use std::process::{Command, Output};

use device_lab::{assert_fails_with, Lab, TEST_PASSWORD};

impl Lab {
    fn srl(name: &str) -> Lab {
        let lab = Lab::new(name);
        lab.write("nobs.toml", "kind = \"lab\"\n");
        lab.write(
            "containerlab/lab.clab.yml",
            "name: nobs-t-dev\ntopology:\n  nodes:\n    nobs-t-srl1:\n      kind: srl\n",
        );
        lab
    }
}

fn nobs_command(here: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command
        .args(args)
        .current_dir(here)
        .env_remove("NOBS_DIR")
        .env("NETWORK_AGENT_USER", "admin")
        .env("NETWORK_AGENT_PASSWORD", TEST_PASSWORD);
    command
}

fn nobs(here: &Path, args: &[&str]) -> Output {
    nobs_command(here, args).output().unwrap()
}

fn flap_command(lab: &Lab, args: &[&str]) -> Command {
    let mut line = vec!["--dir", lab.0.to_str().unwrap(), "device", "flap"];
    line.extend(args);
    nobs_command(&lab.0, &line)
}

fn flap(lab: &Lab, args: &[&str]) -> Output {
    flap_command(lab, args).output().unwrap()
}

fn flap_with_no_login(lab: &Lab, args: &[&str]) -> Output {
    flap_command(lab, args)
        .env_remove("NETWORK_AGENT_USER")
        .env_remove("NETWORK_AGENT_PASSWORD")
        .output()
        .unwrap()
}

#[test]
fn each_unfit_flap_is_refused_with_its_line_and_exit_1_before_any_connection() {
    let lab = Lab::srl("unfit");
    let cases: [(&[&str], &str); 7] = [
        (
            &["nobs-t-srl1", "ethernet-1/1 x", "--yes"],
            "\"ethernet-1/1 x\" is not the name of one interface",
        ),
        (
            &["nobs-t-srl1", "ethernet-1/{1,2}", "--yes"],
            "\"ethernet-1/{1,2}\" is not the name of one interface",
        ),
        (
            &["nobs-t-srl1", "*", "--yes"],
            "\"*\" is not the name of one interface",
        ),
        (
            &["nobs-t-srl1", "mgmt0", "--yes"],
            "mgmt0 is the management interface",
        ),
        (
            &["nobs-t-srl1", "ethernet-1/1", "--yes", "--delay", "301"],
            "the delay must be 1 to 300 seconds",
        ),
        (
            &["nobs-t-srl1", "ethernet-1/1", "--yes", "--delay", "0"],
            "the delay must be 1 to 300 seconds",
        ),
        (
            &["nobs-t-srl1", "ethernet-1/1", "--yes", "--count", "11"],
            "the count must be 1 to 10",
        ),
    ];
    for (args, words) in cases {
        assert_fails_with(
            &flap(&lab, args),
            &format!("nobs: flap nobs-t-srl1: {words}"),
        );
    }
}

#[test]
fn with_no_login_each_unfit_flap_is_refused_with_its_own_line_and_exit_1() {
    let lab = Lab::srl("unfit-no-login");
    let cases: [(&[&str], &str); 4] = [
        (
            &["nobs-t-srl1", "ethernet-1/1 x", "--yes"],
            "\"ethernet-1/1 x\" is not the name of one interface",
        ),
        (
            &["nobs-t-srl1", "mgmt0", "--yes"],
            "mgmt0 is the management interface",
        ),
        (
            &["nobs-t-srl1", "ethernet-1/1", "--yes", "--delay", "301"],
            "the delay must be 1 to 300 seconds",
        ),
        (
            &["nobs-t-srl1", "ethernet-1/1", "--yes", "--count", "11"],
            "the count must be 1 to 10",
        ),
    ];
    for (args, words) in cases {
        assert_fails_with(
            &flap_with_no_login(&lab, args),
            &format!("nobs: flap nobs-t-srl1: {words}"),
        );
    }
}

#[test]
fn with_no_login_a_node_with_no_driver_and_a_fit_flap_keep_their_lines() {
    let lab = Lab::srl("no-driver-no-login");
    lab.write(
        "containerlab/lab.clab.yml",
        "name: nobs-t-dev\ntopology:\n  nodes:\n    nobs-t-srl1:\n      kind: srl\n    nobs-t-ceos1:\n      kind: ceos\n",
    );
    for interface in ["ethernet-1/1 x", "mgmt0", "ethernet-1/1"] {
        assert_fails_with(
            &flap_with_no_login(&lab, &["nobs-t-ceos1", interface, "--yes"]),
            "nobs: ssh nobs-t-ceos1: no driver for the kind ceos yet",
        );
    }
    assert_fails_with(
        &flap_with_no_login(&lab, &["nobs-t-srl1", "ethernet-1/1", "--yes"]),
        "nobs: ssh nobs-t-srl1: set NETWORK_AGENT_USER and NETWORK_AGENT_PASSWORD in the lab's .env file or in the shell",
    );
}

#[test]
fn with_no_terminal_and_no_yes_nothing_is_sent_and_the_line_asks_for_a_yes() {
    let lab = Lab::srl("no-terminal");
    assert_fails_with(
        &flap(&lab, &["nobs-t-srl1", "ethernet-1/1"]),
        "nobs: flap nobs-t-srl1: needs a yes; run it in a terminal, or add --yes",
    );
}

#[test]
fn a_name_the_topology_does_not_hold_names_the_topology_file() {
    let lab = Lab::srl("not-a-node");
    assert_fails_with(
        &flap(&lab, &["nobs-t-x", "ethernet-1/1", "--yes"]),
        &format!(
            "nobs: flap nobs-t-x: not a node of {}",
            lab.0.join("containerlab/lab.clab.yml").display()
        ),
    );
}

#[test]
fn the_flap_help_gives_its_row_its_arguments_and_its_defaults() {
    let here = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let group = nobs(here, &["device", "--help"]);
    let group = String::from_utf8(group.stdout).unwrap();
    assert!(
        group.contains(
            "  flap    Take one interface of a device down after a yes; the device brings it back by itself after the delay"
        ),
        "{group}"
    );

    let help = nobs(here, &["device", "flap", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    let help = String::from_utf8(help.stdout).unwrap();
    for words in [
        "Usage: nobs device flap [OPTIONS] <NAME> <INTERFACE>",
        "  <NAME>       Node name in the lab's topology file",
        "  <INTERFACE>  The device's own name of the interface, such as ethernet-1/1",
        "      --yes              Flap without asking",
        "      --delay <SECONDS>  The time the interface stays down, and up before the next flap; 1 to 300 seconds [default: 5]",
        "      --count <N>        How many times to flap the interface; 1 to 10 [default: 1]",
    ] {
        assert!(help.contains(words), "{help}");
    }
}
