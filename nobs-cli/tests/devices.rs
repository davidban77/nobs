use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

const TOKEN: &str = "secret-token";
const GRAPHQL: &str = "/graphql/main";
const HEADER: &str = "NAME  KIND  SITE  ROLE  MAINT\n";
const NO_SOURCE: &str = "this lab has no source of truth";
const NO_EDGES: &str = r#"{"data":{"WorkshopDevice":{"edges":[]}}}"#;
const TWO_DEVICES: &str = r#"{"data":{"WorkshopDevice":{"edges":[{"node":{"name":{"value":"srl1"},"maintenance":{"value":true},"site_name":{"value":"lab"},"role":{"value":"edge"}}},{"node":{"name":{"value":"srl2"},"maintenance":{"value":false},"site_name":{"value":"lab"},"role":{"value":"edge"}}}]}}}"#;
const NO_STREAMS: &str = r#"{"status":"success","data":{"resultType":"streams","result":[]}}"#;
const NO_SERIES: &str = r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#;

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str, env: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-devices-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture(), &root.join("nobs-t-one"));
        let copy = Self(root.canonicalize().unwrap());
        fs::write(copy.dir().join(".env"), env).unwrap();
        copy
    }

    fn without_source(name: &str, env: &str) -> Self {
        let copy = Self::new(name, env);
        let path = copy.dir().join("nobs.toml");
        let settings = fs::read_to_string(&path).unwrap();
        fs::write(path, format!("sot = \"none\"\n{settings}")).unwrap();
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

struct NoSource {
    copy: Copy,
    infrahub: Stub,
    sonda: Stub,
    observability: Stub,
}

impl NoSource {
    fn new(name: &str) -> Self {
        let infrahub = Stub::start(&[("POST", GRAPHQL, 200, TWO_DEVICES)]);
        let sonda = Stub::start(&[("POST", "/events", 200, "{}")]);
        let observability = Stub::start(&[
            ("GET", "/api/v1/query*", 200, NO_SERIES),
            ("GET", "/loki/api/v1/query_range?*", 200, NO_STREAMS),
        ]);
        let env = format!(
            "INFRAHUB_ADDRESS={}\nINFRAHUB_API_TOKEN={TOKEN}\nSONDA_SERVER_URL={}\nPROMETHEUS_URL={}\nLOKI_URL={}\n",
            infrahub.url(),
            sonda.url(),
            observability.url(),
            observability.url()
        );
        Self {
            copy: Copy::without_source(name, &env),
            infrahub,
            sonda,
            observability,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        run(&self.copy, args)
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

fn closed_port_url() -> String {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}")
}

fn run(copy: &Copy, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command.arg("nobs-t-one").args(args).current_dir(copy.dir());
    for name in [
        "INFRAHUB_ADDRESS",
        "INFRAHUB_API_TOKEN",
        "SONDA_SERVER_URL",
        "SONDA_API_KEY",
        "SONDA_LOKI_SINK_URL",
        "PROMETHEUS_URL",
        "LOKI_URL",
        "NOBS_DIR",
    ] {
        command.env_remove(name);
    }
    command.output().unwrap()
}

fn infrahub_env(url: &str) -> String {
    format!("INFRAHUB_ADDRESS={url}\nINFRAHUB_API_TOKEN={TOKEN}\n")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn devices_prints_the_header_then_one_row_per_device_with_a_dash_for_a_missing_value() {
    let infrahub = Stub::start(&[("POST", GRAPHQL, 200, TWO_DEVICES)]);
    let copy = Copy::new("two-devices", &infrahub_env(infrahub.url()));

    let output = run(&copy, &["devices"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        format!("{HEADER}srl1  —     lab   edge  yes\nsrl2  —     lab   edge  no\n")
    );
    assert_eq!(infrahub.requests().len(), 1);
}

#[test]
fn devices_prints_the_header_alone_when_the_source_has_no_device() {
    let infrahub = Stub::start(&[("POST", GRAPHQL, 200, NO_EDGES)]);
    let copy = Copy::new("no-device", &infrahub_env(infrahub.url()));

    let output = run(&copy, &["devices"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(stdout(&output), HEADER);
}

#[test]
fn devices_prints_the_error_alone_and_exits_1_when_infrahub_cannot_be_reached() {
    let closed = closed_port_url();
    let copy = Copy::new("closed-port", &infrahub_env(&closed));

    let output = run(&copy, &["devices"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).starts_with(&format!("nobs: cannot reach {closed}: ")),
        "{}",
        stderr(&output)
    );
}

#[test]
fn devices_under_no_source_prints_the_header_alone_and_asks_infrahub_nothing() {
    let lab = NoSource::new("none-devices");

    let output = lab.run(&["devices"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(stdout(&output), HEADER);
    assert_eq!(lab.infrahub.requests().len(), 0);
}

#[test]
fn evidence_under_no_source_prints_the_no_source_form_and_asks_infrahub_nothing() {
    let lab = NoSource::new("none-evidence");

    let output = lab.run(&["evidence", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let text = stdout(&output);
    let blocks: Vec<&str> = text.trim().split("\n\n").collect();
    assert_eq!(blocks.len(), 4, "{text}");
    assert_eq!(blocks[0], format!("Source of truth\n{NO_SOURCE}."));
    assert_eq!(
        blocks[3],
        format!("Policy hint\ndecision: stop\nreason  : {NO_SOURCE}")
    );
    assert_eq!(lab.infrahub.requests().len(), 0);
    assert_eq!(lab.observability.requests().len(), 7);
}

#[test]
fn maintenance_under_no_source_says_so_exits_1_and_posts_nothing_to_sonda() {
    let lab = NoSource::new("none-maintenance");

    let output = lab.run(&["maintenance", "-d", "srl1"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), format!("nobs: {NO_SOURCE}\n"));
    assert_eq!(lab.infrahub.requests().len(), 0);
    assert_eq!(lab.sonda.requests().len(), 0);
}
