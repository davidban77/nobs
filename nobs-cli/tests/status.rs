use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

const SERVICES: [(&str, &str); 7] = [
    ("Grafana", "--grafana-url"),
    ("Prometheus", "--prom-url"),
    ("Loki", "--loki-url"),
    ("Alertmanager", "--am-url"),
    ("Infrahub", "--infrahub-url"),
    ("Prefect", "--prefect-url"),
    ("Sonda", "--sonda-url"),
];

const CLOSING: &str =
    "Some services are not ready yet: give them a minute, or check nobs nobs-t-one logs.";

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-status-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture(), &root.join("nobs-t-one"));
        Self(root)
    }

    fn dir(&self) -> PathBuf {
        self.0.join("nobs-t-one")
    }

    fn write_env(&self, text: &str) {
        fs::write(self.dir().join(".env"), text).unwrap();
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

fn healthy_stub() -> Stub {
    Stub::start(&[
        ("GET", "/api/health", 200, ""),
        ("GET", "/-/ready", 200, ""),
        ("GET", "/ready", 200, ""),
        ("GET", "/health", 200, ""),
    ])
}

fn flags_at(url: &str, except: &[&str]) -> Vec<String> {
    SERVICES
        .iter()
        .filter(|(service, _)| !except.contains(service))
        .flat_map(|(_, flag)| [flag.to_string(), url.to_string()])
        .collect()
}

fn closed_port_url() -> String {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}")
}

fn nobs(copy: &Copy, args: &[String], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command
        .args(["nobs-t-one", "status"])
        .args(args)
        .env_remove("PROMETHEUS_URL")
        .env_remove("LOKI_URL")
        .env_remove("ALERTMANAGER_URL")
        .env_remove("INFRAHUB_ADDRESS")
        .env_remove("NOBS_DIR")
        .current_dir(copy.dir());
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn row(stdout: &str, service: &str) -> (String, String) {
    let line = stdout
        .lines()
        .find(|line| line.starts_with(service))
        .unwrap_or_else(|| panic!("no row for {service}\n{stdout}"));
    let (url, state) = line[service.len()..]
        .trim_start()
        .split_once("  ")
        .unwrap_or_else(|| panic!("no state in {line}"));
    (url.to_string(), state.trim().to_string())
}

#[test]
fn status_prints_a_header_and_seven_ok_rows_and_exits_0() {
    let copy = Copy::new("all-ok");
    let stub = healthy_stub();
    let url = stub.url();

    let output = nobs(&copy, &flags_at(url, &[]), &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let pad = " ".repeat(url.len() - 3);
    assert_eq!(
        stdout(&output),
        format!(
            "Service       URL{pad}  State\n\
             Grafana       {url}  ok\n\
             Prometheus    {url}  ok\n\
             Loki          {url}  ok\n\
             Alertmanager  {url}  ok\n\
             Infrahub      {url}  ok\n\
             Prefect       {url}  ok\n\
             Sonda         {url}  ok\n"
        )
    );
}

#[test]
fn a_503_row_reads_http_503_and_the_closing_line_names_the_workshop_and_the_exit_is_1() {
    let copy = Copy::new("one-503");
    let stub = Stub::start(&[
        ("GET", "/api/health", 200, ""),
        ("GET", "/-/ready", 200, ""),
        ("GET", "/ready", 503, ""),
        ("GET", "/health", 200, ""),
    ]);

    let output = nobs(&copy, &flags_at(stub.url(), &[]), &[]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let out = stdout(&output);
    assert_eq!(
        row(&out, "Loki"),
        (stub.url().to_string(), "http 503".to_string())
    );
    assert_eq!(row(&out, "Grafana").1, "ok");
    assert_eq!(out.lines().last(), Some(CLOSING));
    assert_eq!(out.lines().count(), 9);
}

#[test]
fn a_closed_port_row_reads_down_and_the_exit_is_1() {
    let copy = Copy::new("closed-port");
    let stub = healthy_stub();
    let closed = closed_port_url();

    let mut args = flags_at(stub.url(), &["Prometheus"]);
    args.extend(["--prom-url".to_string(), closed.clone()]);
    let output = nobs(&copy, &args, &[]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let out = stdout(&output);
    let (url, state) = row(&out, "Prometheus");
    assert_eq!(url, closed);
    assert!(state.starts_with("down ("), "{state}");
    assert_eq!(out.lines().last(), Some(CLOSING));
}

#[test]
fn the_dot_env_file_gives_the_prometheus_address_and_the_infrahub_address_is_rewritten() {
    let copy = Copy::new("dot-env");
    let stub = healthy_stub();
    copy.write_env(&format!(
        "PROMETHEUS_URL={}\nINFRAHUB_ADDRESS=http://infrahub-server:8000\n",
        stub.url()
    ));

    let args = flags_at(stub.url(), &["Prometheus", "Infrahub"]);
    let output = nobs(&copy, &args, &[]);

    let out = stdout(&output);
    assert_eq!(
        row(&out, "Prometheus"),
        (stub.url().to_string(), "ok".to_string())
    );
    assert_eq!(row(&out, "Infrahub").0, "http://localhost:8000");
}

#[test]
fn the_infrahub_flag_beats_the_dot_env_file() {
    let copy = Copy::new("flag-beats-env");
    let stub = healthy_stub();
    let other = healthy_stub();
    copy.write_env("INFRAHUB_ADDRESS=http://infrahub-server:8000\n");

    let mut args = flags_at(stub.url(), &["Infrahub"]);
    args.extend(["--infrahub-url".to_string(), other.url().to_string()]);
    let output = nobs(&copy, &args, &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        row(&stdout(&output), "Infrahub"),
        (other.url().to_string(), "ok".to_string())
    );
}

#[test]
fn a_blank_flag_counts_as_not_given_and_the_dot_env_file_is_used() {
    let copy = Copy::new("blank-flag");
    let stub = healthy_stub();
    copy.write_env(&format!("PROMETHEUS_URL={}\n", stub.url()));

    let mut args = flags_at(stub.url(), &["Prometheus"]);
    args.extend(["--prom-url".to_string(), String::new()]);
    let output = nobs(&copy, &args, &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        row(&stdout(&output), "Prometheus"),
        (stub.url().to_string(), "ok".to_string())
    );
}

#[test]
fn a_blank_shell_value_does_not_hide_the_dot_env_file() {
    let copy = Copy::new("blank-shell");
    let stub = healthy_stub();
    copy.write_env(&format!(
        "PROMETHEUS_URL={url}\nLOKI_URL={url}\nALERTMANAGER_URL={url}\nINFRAHUB_ADDRESS={url}\n",
        url = stub.url()
    ));

    let args = flags_at(
        stub.url(),
        &["Prometheus", "Loki", "Alertmanager", "Infrahub"],
    );
    let output = nobs(
        &copy,
        &args,
        &[
            ("PROMETHEUS_URL", " "),
            ("LOKI_URL", " "),
            ("ALERTMANAGER_URL", " "),
            ("INFRAHUB_ADDRESS", " "),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let out = stdout(&output);
    for service in ["Prometheus", "Loki", "Alertmanager", "Infrahub"] {
        assert_eq!(
            row(&out, service),
            (stub.url().to_string(), "ok".to_string()),
            "{service}"
        );
    }
}
