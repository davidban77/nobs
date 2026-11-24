use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TEST_PASSWORD: &str = "nobs-t-not-a-secret";

const TEMPLATE: &str =
    "set / system name host-name {{ name }}\nset / interface mgmt0 admin-state enable\n";

struct Lab(PathBuf);

impl Lab {
    fn new(name: &str) -> Lab {
        let root =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-device-push-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Lab(root.canonicalize().unwrap())
    }

    fn with(name: &str, kind: &str, template: &str) -> Lab {
        let lab = Lab::new(name);
        lab.write("nobs.toml", "kind = \"lab\"\n");
        lab.write(
            "containerlab/lab.clab.yml",
            &format!(
                "name: nobs-t-dev\ntopology:\n  nodes:\n    nobs-t-srl1:\n      kind: {kind}\n"
            ),
        );
        lab.write(
            "containerlab/lab_vars.yml",
            "nodes:\n  nobs-t-srl1:\n    role: nobs-t-leaf\n",
        );
        lab.write("templates/srl.j2", template);
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

fn nobs(here: &Path, args: &[&str], password: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command.args(args).current_dir(here).env_remove("NOBS_DIR");
    match password {
        Some(password) => command
            .env("NETWORK_AGENT_USER", "admin")
            .env("NETWORK_AGENT_PASSWORD", password),
        None => command
            .env_remove("NETWORK_AGENT_USER")
            .env_remove("NETWORK_AGENT_PASSWORD"),
    };
    command.output().unwrap()
}

fn push(dir: &Path, args: &[&str], password: Option<&str>) -> Output {
    let mut line = vec!["--dir", dir.to_str().unwrap(), "device", "push"];
    line.extend(args);
    nobs(dir, &line, password)
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
    assert!(!stderr.contains(TEST_PASSWORD));
}

#[test]
fn a_kind_with_no_template_is_not_pushed_and_names_the_template() {
    let lab = Lab::with("no-template", "linux", TEMPLATE);
    assert_fails_with(
        &push(&lab.0, &["nobs-t-srl1"], None),
        "nobs: push nobs-t-srl1: no template for kind linux: expected templates/linux.j2",
    );
}

#[test]
fn an_error_in_the_intended_text_is_not_pushed_and_gives_the_template_words_on_one_line() {
    let lab = Lab::with("text-error", "srl", "{{ nobs_t_missing }}\n");

    let output = push(&lab.0, &["nobs-t-srl1", "--yes"], None);

    let stderr = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(output.stdout.is_empty());
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.ends_with('\n'), "{stderr}");
    assert!(
        stderr.starts_with("nobs: push nobs-t-srl1: templates/srl.j2: "),
        "{stderr}"
    );
    assert!(stderr.contains("nobs_t_missing"), "{stderr}");
}

#[test]
fn an_empty_intended_text_is_not_pushed() {
    let lab = Lab::with("empty", "srl", "\n  \n");
    assert_fails_with(
        &push(&lab.0, &["nobs-t-srl1", "--yes"], None),
        "nobs: push nobs-t-srl1: the intended configuration has no line",
    );
}

#[test]
fn a_name_the_topology_does_not_hold_names_the_topology_file() {
    let lab = Lab::with("not-a-node", "srl", TEMPLATE);
    assert_fails_with(
        &push(&lab.0, &["nobs-t-x", "--yes"], Some(TEST_PASSWORD)),
        &format!(
            "nobs: push nobs-t-x: not a node of {}",
            lab.0.join("containerlab/lab.clab.yml").display()
        ),
    );
}

#[test]
fn outside_a_lab_or_in_a_workshop_the_lines_of_today() {
    let bare = Lab::new("no-lab");
    assert_fails_with(
        &nobs(
            &bare.0,
            &["device", "push", "nobs-t-srl1", "--yes"],
            Some(TEST_PASSWORD),
        ),
        "nobs: no lab or workshop found from here",
    );

    let workshop = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/workshop-one");
    assert_fails_with(
        &push(&workshop, &["nobs-t-srl1", "--yes"], Some(TEST_PASSWORD)),
        &format!(
            "nobs: device commands need a lab with containerlab nodes; {} is a workshop",
            workshop.display()
        ),
    );
}

#[test]
fn with_no_login_set_nothing_is_sent_and_the_line_names_both_variables() {
    let lab = Lab::with("no-login", "srl", TEMPLATE);
    assert_fails_with(
        &push(&lab.0, &["nobs-t-srl1", "--yes"], None),
        "nobs: ssh nobs-t-srl1: set NETWORK_AGENT_USER and NETWORK_AGENT_PASSWORD in the lab's .env file or in the shell",
    );
}

#[test]
fn with_no_terminal_and_no_yes_nothing_is_sent_and_the_line_asks_for_a_yes() {
    let lab = Lab::with("no-terminal", "srl", TEMPLATE);
    assert_fails_with(
        &push(&lab.0, &["nobs-t-srl1"], Some(TEST_PASSWORD)),
        "nobs: push nobs-t-srl1: needs a yes; run it in a terminal, or add --yes",
    );
}

fn help(args: &[&str]) -> String {
    let mut line = args.to_vec();
    line.push("--help");
    let output = nobs(Path::new(env!("CARGO_TARGET_TMPDIR")), &line, None);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    String::from_utf8(output.stdout).unwrap()
}

fn save_line(help: &str) -> &str {
    help.lines()
        .find(|line| line.trim_start().starts_with("--save"))
        .unwrap()
}

#[test]
fn the_push_help_gives_its_row_its_arguments_and_the_save_words_of_check() {
    let group = help(&["device"]);
    assert!(
        group.contains(
            "  push    Send the intended configuration to a device in one commit, after a yes, and fetch it again"
        ),
        "{group}"
    );

    let push = help(&["device", "push"]);
    for words in [
        "Usage: nobs device push [OPTIONS] <NAME>",
        "  <NAME>  Node name in the lab's topology file",
        "      --yes            Push without asking",
        "      --save <FOLDER>  Also write what each device answered to a file in this folder; the folder must not exist",
    ] {
        assert!(push.contains(words), "{push}");
    }
    assert_eq!(save_line(&push), save_line(&help(&["device", "check"])));
}
