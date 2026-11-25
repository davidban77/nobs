use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TEST_PASSWORD: &str = "nobs-t-not-a-secret";

struct Lab(PathBuf);

impl Lab {
    fn new(name: &str) -> Lab {
        let root =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-device-flap-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let lab = Lab(root.canonicalize().unwrap());
        lab.write("nobs.toml", "kind = \"lab\"\n");
        lab.write(
            "containerlab/lab.clab.yml",
            "name: nobs-t-dev\ntopology:\n  nodes:\n    nobs-t-srl1:\n      kind: srl\n",
        );
        lab
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn nobs(here: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(args)
        .current_dir(here)
        .env_remove("NOBS_DIR")
        .env("NETWORK_AGENT_USER", "admin")
        .env("NETWORK_AGENT_PASSWORD", TEST_PASSWORD)
        .output()
        .unwrap()
}

fn flap(lab: &Lab, args: &[&str]) -> Output {
    let mut line = vec!["--dir", lab.0.to_str().unwrap(), "device", "flap"];
    line.extend(args);
    nobs(&lab.0, &line)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_fails_with(output: &Output, line: &str) {
    let stderr = stderr(output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr, format!("{line}\n"));
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn each_unfit_flap_is_refused_with_its_line_and_exit_1_before_any_connection() {
    let lab = Lab::new("unfit");
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
fn with_no_terminal_and_no_yes_nothing_is_sent_and_the_line_asks_for_a_yes() {
    let lab = Lab::new("no-terminal");
    assert_fails_with(
        &flap(&lab, &["nobs-t-srl1", "ethernet-1/1"]),
        "nobs: flap nobs-t-srl1: needs a yes; run it in a terminal, or add --yes",
    );
}

#[test]
fn a_name_the_topology_does_not_hold_names_the_topology_file() {
    let lab = Lab::new("not-a-node");
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
        "      --delay <SECONDS>  The time the interface stays down, and up before the next flap [default: 5]",
        "      --count <N>        How many times to flap the interface [default: 1]",
    ] {
        assert!(help.contains(words), "{help}");
    }
}
