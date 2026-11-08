use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

const TOKEN: &str = "nobs-t-token";
const COMMANDS: [&str; 2] = ["load-nautobot", "delete-nautobot"];
const NO_RESULTS: &str = r#"{"count":0,"next":null,"results":[]}"#;
const CREATED: &str = r#"{"id":"new-1"}"#;
const TOKEN_REQUIRED: &str =
    "NAUTOBOT_SUPERUSER_API_TOKEN is required (set it in .env or pass --nautobot-token)";

const PROGRESS: &str = "role network_device created
manufacturer Arista created
device type cEOS created
manufacturer Nokia created
device type SRLinux created
location type site created
status lab-active created
status Alerted created
custom field maintenance created
location lab created
namespace lab-default created
prefix 192.0.2.0/24 created
prefix 203.0.113.0/24 created
device nobs-t-srl1 created
interface ethernet-1/1 created
ip address 192.0.2.0/31 created
ip address to interface 192.0.2.0/31 on ethernet-1/1 created
ip address 203.0.113.11 created
interface Management0 created
ip address to interface 203.0.113.11 on Management0 created
device nobs-t-srl2 created
interface ethernet-1/1 created
ip address 192.0.2.1/31 created
ip address to interface 192.0.2.1/31 on ethernet-1/1 created
ip address 203.0.113.12 created
interface Management0 created
ip address to interface 203.0.113.12 on Management0 created
";

const TABLE: &str = "Object type              Created  Updated  Total
role                     1        0        1
manufacturer             2        0        2
device type              2        0        2
location type            1        0        1
status                   2        0        2
custom field             1        0        1
location                 1        0        1
namespace                1        0        1
prefix                   2        0        2
device                   2        0        2
interface                4        0        4
ip address               4        0        4
ip address to interface  4        0        4
";

const DELETED: &str = "ip address to interface: 1 deleted
ip address: 1 deleted
device: 1 deleted
prefix: 2 deleted
namespace: 1 deleted
location: 1 deleted
location type: 0 deleted
status: 2 deleted
custom field: 0 deleted
device type: 0 deleted
manufacturer: 0 deleted
role: 0 deleted
";

const OTHER_TOPOLOGY: &str = "name: nobs-t-other
mgmt:
  ipv4-subnet: 203.0.113.0/24
topology:
  nodes:
    nobs-t-leaf1:
      kind: nokia_srlinux
";

const OTHER_VARS: &str = "prefixes:
  - prefix: 192.0.2.0/24
    name: nobs-t-links
nodes:
  nobs-t-leaf1:
    mgmt-ipv4: 203.0.113.21
    interfaces:
      - name: ethernet-1/1
        ipv4: 192.0.2.2/31
        role: nobs-t-peer
";

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-utils-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture("book-lab"), &root.join("nobs-t-book"));
        let copy = Self(root.canonicalize().unwrap());
        copy.write(".env", &format!("NAUTOBOT_SUPERUSER_API_TOKEN={TOKEN}\n"));
        let topology = fs::read_to_string(copy.dir().join("containerlab/lab.yml")).unwrap();
        copy.write(
            "containerlab/lab.yml",
            &topology.replace(
                "  network: nobs-t-net\n",
                "  network: nobs-t-net\n  ipv4-subnet: 203.0.113.0/24\n",
            ),
        );
        copy
    }

    fn root(&self) -> &Path {
        &self.0
    }

    fn dir(&self) -> PathBuf {
        self.0.join("nobs-t-book")
    }

    fn shown(&self) -> String {
        self.dir().display().to_string()
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.dir().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(name)
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

fn nobs(cwd: &Path, args: &[&str]) -> Output {
    nobs_env(cwd, args, &[])
}

fn nobs_env(cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command.args(args).current_dir(cwd);
    for name in ["NAUTOBOT_URL", "NAUTOBOT_SUPERUSER_API_TOKEN", "NOBS_DIR"] {
        command.env_remove(name);
    }
    command.envs(env.iter().copied()).output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn nautobot() -> Stub {
    Stub::start(&[
        ("GET", "/api/*", 200, NO_RESULTS),
        ("POST", "/api/*", 200, CREATED),
        ("PATCH", "/api/dcim/devices/*", 200, "{}"),
    ])
}

fn nautobot_with_a_bad_device() -> Stub {
    Stub::start(&[
        ("GET", "/api/*", 200, NO_RESULTS),
        ("POST", "/api/dcim/devices/", 400, "bad device\nsecond line"),
        ("POST", "/api/*", 200, CREATED),
    ])
}

fn nautobot_with_objects(refused: &[(&str, &str, u16, &str)]) -> Stub {
    let page = |ids: &[&str]| {
        let items: Vec<String> = ids.iter().map(|id| format!(r#"{{"id":"{id}"}}"#)).collect();
        format!(r#"{{"next":null,"results":[{}]}}"#, items.join(","))
    };
    let devices = page(&["dev-1"]);
    let interfaces = page(&["if-1"]);
    let prefixes = page(&["pre-1", "pre-2"]);
    let addresses = page(&["ip-1"]);
    let active = page(&["st-1"]);
    let alerted = page(&["st-2"]);
    let locations = page(&["loc-1"]);
    let namespaces = page(&["ns-1"]);
    let mappings = r#"{"next":null,"results":[{"id":"map-1","ip_address":{"id":"ip-1"}}]}"#;
    let mut routes: Vec<(&str, &str, u16, &str)> = vec![
        ("GET", "/api/dcim/devices/*", 200, &devices),
        ("GET", "/api/dcim/interfaces/*", 200, &interfaces),
        ("GET", "/api/ipam/ip-address-to-interface/*", 200, mappings),
        ("GET", "/api/ipam/prefixes/*", 200, &prefixes),
        ("GET", "/api/ipam/ip-addresses/*", 200, &addresses),
        (
            "GET",
            "/api/extras/statuses/?name=lab-active*",
            200,
            &active,
        ),
        ("GET", "/api/extras/statuses/?name=Alerted*", 200, &alerted),
        ("GET", "/api/dcim/locations/*", 200, &locations),
        ("GET", "/api/ipam/namespaces/*", 200, &namespaces),
        ("GET", "/api/*", 200, r#"{"next":null,"results":[]}"#),
    ];
    routes.extend_from_slice(refused);
    routes.push(("DELETE", "/api/*", 204, ""));
    Stub::start(&routes)
}

fn device_posts(stub: &Stub) -> Vec<String> {
    stub.requests()
        .iter()
        .filter(|request| request.method == "POST" && request.path == "/api/dcim/devices/")
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect()
}

#[test]
fn load_nautobot_prints_each_object_the_table_and_the_total() {
    let stub = nautobot();
    let copy = Copy::new("load");

    let output = nobs(
        &copy.dir(),
        &["utils", "load-nautobot", "--nautobot-url", stub.url()],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        format!(
            "{PROGRESS}{TABLE}Done: 27 objects across 13 types. Open Nautobot at {}.\n",
            stub.url()
        )
    );
    let seen = stub.requests();
    assert!(!seen.is_empty());
    for request in &seen {
        assert_eq!(
            request.header("Authorization"),
            Some("Token nobs-t-token"),
            "{} {}",
            request.method,
            request.path
        );
    }
}

#[test]
fn without_a_token_both_commands_say_so_and_send_nothing() {
    let stub = nautobot();
    let copy = Copy::new("no-token");
    copy.write(".env", "");

    for name in COMMANDS {
        let output = nobs(&copy.dir(), &["utils", name, "--nautobot-url", stub.url()]);

        assert_eq!(output.status.code(), Some(1), "{name}");
        assert_eq!(
            stderr(&output),
            format!("nobs: utils {name}: {TOKEN_REQUIRED}\n"),
            "{name}"
        );
        assert_eq!(stdout(&output), "", "{name}");
    }
    assert_eq!(stub.requests().len(), 0);
}

#[test]
fn a_refused_write_exits_1_with_the_request_and_prints_no_table_and_no_done() {
    let stub = nautobot_with_a_bad_device();
    let copy = Copy::new("refused");

    let output = nobs(
        &copy.dir(),
        &["utils", "load-nautobot", "--nautobot-url", stub.url()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: utils load-nautobot: POST /api/dcim/devices/ failed: 400 bad device\n"
    );
    let out = stdout(&output);
    assert!(out.contains("location lab created\n"), "{out}");
    for later in ["Object type", "Done:"] {
        assert!(!out.contains(later), "{out}");
    }
}

#[test]
fn a_nautobot_that_cannot_be_reached_exits_1_with_the_address_and_the_reason() {
    let closed = closed_port_url();
    let copy = Copy::new("unreachable");

    let output = nobs(
        &copy.dir(),
        &["utils", "load-nautobot", "--nautobot-url", &closed],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with(&format!(
            "nobs: utils load-nautobot: cannot reach {closed}: "
        )),
        "{}",
        stderr(&output)
    );
    assert!(!stdout(&output).contains("Done:"));
}

#[test]
fn an_https_address_exits_1_with_the_tls_line_and_sends_nothing() {
    let stub = nautobot();
    let copy = Copy::new("https");
    let url = stub.url().replacen("http://", "https://", 1);

    for name in COMMANDS {
        let output = nobs(&copy.dir(), &["utils", name, "--nautobot-url", &url]);

        assert_eq!(output.status.code(), Some(1), "{name}");
        assert_eq!(
            stderr(&output),
            format!(
                "nobs: utils {name}: {url}: this build of nobs has no TLS; use an http:// URL\n"
            ),
            "{name}"
        );
        assert_eq!(stdout(&output), "", "{name}");
    }
    assert_eq!(stub.requests().len(), 0);
}

#[test]
fn a_blank_token_in_the_shell_lets_the_dot_env_token_through() {
    let stub = nautobot();
    let copy = Copy::new("blank-shell-token");

    let output = nobs_env(
        &copy.dir(),
        &["utils", "load-nautobot", "--nautobot-url", stub.url()],
        &[("NAUTOBOT_SUPERUSER_API_TOKEN", "  ")],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!stub.requests().is_empty());
}

#[test]
fn delete_nautobot_prints_each_kind_and_the_total() {
    let stub = nautobot_with_objects(&[]);
    let copy = Copy::new("delete");

    let output = nobs(
        &copy.dir(),
        &["utils", "delete-nautobot", "--nautobot-url", stub.url()],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        format!("{DELETED}Done: 9 objects deleted.\n")
    );
}

#[test]
fn a_refused_delete_exits_1_with_the_request_and_prints_no_done() {
    let stub = nautobot_with_objects(&[(
        "DELETE",
        "/api/dcim/devices/",
        500,
        "cannot delete\nsecond line",
    )]);
    let copy = Copy::new("refused-delete");

    let output = nobs(
        &copy.dir(),
        &["utils", "delete-nautobot", "--nautobot-url", stub.url()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: utils delete-nautobot: DELETE /api/dcim/devices/ failed: 500 cannot delete\n"
    );
    assert_eq!(
        stdout(&output),
        "ip address to interface: 1 deleted\nip address: 1 deleted\n"
    );
}

#[test]
fn the_default_files_are_taken_from_the_located_folder_not_the_current_one() {
    let stub = nautobot();
    let copy = Copy::new("defaults");

    let output = nobs(
        copy.root(),
        &[
            "--dir",
            &copy.shown(),
            "utils",
            "load-nautobot",
            "--nautobot-url",
            stub.url(),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let devices = device_posts(&stub);
    assert_eq!(devices.len(), 2);
    assert!(
        devices[0].contains(r#""name":"nobs-t-srl1""#),
        "{}",
        devices[0]
    );
    assert!(
        devices[1].contains(r#""name":"nobs-t-srl2""#),
        "{}",
        devices[1]
    );
}

#[test]
fn a_relative_topology_and_vars_path_is_taken_from_the_located_folder() {
    let stub = nautobot();
    let copy = Copy::new("relative");
    copy.write("containerlab/other.yml", OTHER_TOPOLOGY);
    copy.write("containerlab/other_vars.yml", OTHER_VARS);

    let output = nobs(
        copy.root(),
        &[
            "--dir",
            &copy.shown(),
            "utils",
            "load-nautobot",
            "--nautobot-url",
            stub.url(),
            "--topology",
            "containerlab/other.yml",
            "--extra-topology-vars",
            "containerlab/other_vars.yml",
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let devices = device_posts(&stub);
    assert_eq!(devices.len(), 1);
    assert!(
        devices[0].contains(r#""name":"nobs-t-leaf1""#),
        "{}",
        devices[0]
    );
}

#[test]
fn a_file_that_cannot_be_read_exits_1_and_sends_nothing() {
    let stub = nautobot();
    let copy = Copy::new("missing");

    for flag in ["--topology", "--extra-topology-vars"] {
        let output = nobs(
            &copy.dir(),
            &[
                "utils",
                "load-nautobot",
                "--nautobot-url",
                stub.url(),
                flag,
                "containerlab/missing.yml",
            ],
        );

        assert_eq!(output.status.code(), Some(1), "{flag}");
        assert!(
            stderr(&output).starts_with(&format!(
                "nobs: utils load-nautobot: cannot read {}/containerlab/missing.yml: ",
                copy.shown()
            )),
            "{flag}: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output), "", "{flag}");
    }
    assert_eq!(stub.requests().len(), 0);
}

#[test]
fn a_workshop_folder_is_refused_by_both_commands() {
    let stub = nautobot();
    let workshop = fixture("workshop-one").canonicalize().unwrap();

    for name in COMMANDS {
        let output = nobs(
            &workshop,
            &[
                "utils",
                name,
                "--nautobot-url",
                stub.url(),
                "--nautobot-token",
                TOKEN,
            ],
        );

        assert_eq!(output.status.code(), Some(1), "{name}");
        assert_eq!(
            stderr(&output),
            format!(
                "nobs: utils {name} needs the book lab (chapters/ and containerlab/lab.yml); {} is a workshop\n",
                workshop.display()
            ),
            "{name}"
        );
        assert_eq!(stdout(&output), "", "{name}");
    }
    assert_eq!(stub.requests().len(), 0);
}
