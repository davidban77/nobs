use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

const TOKEN: &str = "secret-token";
const TOKEN_REQUIRED: &str =
    "nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).\n";
const LIMIT: Duration = Duration::from_secs(20);

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str, env: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-preflight-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture(), &root.join("nobs-t-one"));
        let copy = Self(root.canonicalize().unwrap());
        fs::write(copy.dir().join(".env"), env).unwrap();
        copy
    }

    fn dir(&self) -> PathBuf {
        self.0.join("nobs-t-one")
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/workshop-one")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn env_for(stub: &Stub, token: bool) -> String {
    let url = stub.url();
    let token = if token {
        format!("INFRAHUB_API_TOKEN={TOKEN}\n")
    } else {
        String::new()
    };
    format!(
        "INFRAHUB_ADDRESS={url}\n{token}PROMETHEUS_URL={url}\nLOKI_URL={url}\nGRAFANA_URL={url}\nSONDA_SERVER_URL={url}\n"
    )
}

fn run(copy: &Copy, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command
        .args(["nobs-t-one", "preflight"])
        .args(args)
        .current_dir(copy.dir())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in [
        "INFRAHUB_ADDRESS",
        "INFRAHUB_API_TOKEN",
        "PROMETHEUS_URL",
        "LOKI_URL",
        "GRAFANA_URL",
        "GRAFANA_USER",
        "GRAFANA_PASSWORD",
        "PREFLIGHT_OUT_DIR",
        "SONDA_SERVER_URL",
        "SONDA_API_KEY",
        "SONDA_LOKI_SINK_URL",
        "NOBS_DIR",
    ] {
        command.env_remove(name);
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + LIMIT;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("preflight was still running after {LIMIT:?}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn a_file_as_the_out_dir(copy: &Copy) -> PathBuf {
    let file = copy.dir().join("not-a-folder");
    fs::write(&file, "x").unwrap();
    file
}

fn assert_cannot_write(output: &Output, path: &Path) {
    let prefix = format!("nobs: cannot write {}: ", path.display());
    let error = stderr(output);
    assert!(error.starts_with(&prefix), "{error}");
    assert!(
        error.ends_with('\n') && error.lines().count() == 1,
        "{error}"
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(output), "");
}

#[test]
fn the_help_page_names_the_command_line_and_the_eight_flags() {
    let stub = Stub::start(&[]);
    let copy = Copy::new("help", &env_for(&stub, true));

    let output = run(&copy, &["--help"]);

    let help = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        help.contains("Run layers A and B of the workshop's check against the live stack"),
        "{help}"
    );
    for flag in [
        "--skip-c",
        "--out-dir <DIR>",
        "--prom-url <URL>",
        "--loki-url <URL>",
        "--grafana-url <URL>",
        "--sonda-url <URL>",
        "--infrahub-url <URL>",
        "--token <TOKEN>",
    ] {
        assert!(
            help.lines().any(|line| line.trim_start().starts_with(flag)),
            "{flag}\n{help}"
        );
    }
}

#[test]
fn an_out_dir_that_is_a_file_is_reported_and_nothing_is_requested() {
    let stub = Stub::start(&[]);
    let copy = Copy::new("file", &env_for(&stub, true));
    let file = a_file_as_the_out_dir(&copy);

    let output = run(&copy, &["--out-dir", file.to_str().unwrap()]);

    assert_cannot_write(&output, &file);
    assert!(stub.requests().is_empty());
}

#[test]
fn skip_c_changes_neither_the_output_nor_the_exit_code() {
    let stub = Stub::start(&[]);
    let copy = Copy::new("skip", &env_for(&stub, true));
    let file = a_file_as_the_out_dir(&copy);
    let out_dir = file.to_str().unwrap();

    let without = run(&copy, &["--out-dir", out_dir]);
    let with = run(&copy, &["--skip-c", "--out-dir", out_dir]);

    assert_cannot_write(&with, &file);
    assert_eq!(stdout(&with), stdout(&without));
    assert_eq!(stderr(&with), stderr(&without));
    assert_eq!(with.status.code(), without.status.code());
    assert!(stub.requests().is_empty());
}

#[test]
fn no_token_prints_the_required_line_before_the_folder_is_touched() {
    let stub = Stub::start(&[]);
    let copy = Copy::new("token", &env_for(&stub, false));
    let file = a_file_as_the_out_dir(&copy);

    let output = run(&copy, &["--out-dir", file.to_str().unwrap()]);

    assert_eq!(stderr(&output), TOKEN_REQUIRED);
    assert_eq!(stdout(&output), "");
    assert_eq!(output.status.code(), Some(1));
    assert!(stub.requests().is_empty());
}

#[test]
fn a_workshop_without_dashboards_is_reported_and_nothing_is_requested() {
    let stub = Stub::start(&[]);
    let copy = Copy::new("dashboards", &env_for(&stub, true));
    let dashboards = copy.dir().join("grafana").join("dashboards");
    fs::remove_dir_all(&dashboards).unwrap();
    let out_dir = copy.dir().join("out");

    let output = run(&copy, &["--out-dir", out_dir.to_str().unwrap()]);

    let error = stderr(&output);
    assert!(
        error.starts_with(&format!("nobs: cannot read {}: ", dashboards.display())),
        "{error}"
    );
    assert!(
        error.ends_with('\n') && error.lines().count() == 1,
        "{error}"
    );
    assert_eq!(stdout(&output), "");
    assert_eq!(output.status.code(), Some(1));
    assert!(stub.requests().is_empty());
}

#[test]
fn with_c_is_a_usage_error() {
    let stub = Stub::start(&[]);
    let copy = Copy::new("with-c", &env_for(&stub, true));

    let output = run(&copy, &["--with-c"]);

    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("--with-c"), "{}", stderr(&output));
    assert!(stub.requests().is_empty());
}
