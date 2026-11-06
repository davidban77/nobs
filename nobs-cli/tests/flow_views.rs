use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::{Seen, Stub};

const TOKEN: &str = "secret-token";
const GRAPHQL: &str = "/graphql/main";
const LOKI_PATH: &str = "/loki/api/v1/query_range";
const LOKI_ROUTE: &str = "/loki/api/v1/query_range?*";
const PROMETHEUS_ROUTE: &str = "/api/v1/query*";
const REWRITTEN: &str = "INFRAHUB_ADDRESS rewritten to host-reachable http://localhost:8000\n";
const TOKEN_REQUIRED: &str =
    "nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).\n";
const NANOSECONDS_PER_MINUTE: i64 = 60_000_000_000;

const OLD_QUERY: &str = r#"evidence_query = "{device=\"srl1\"}""#;
const NEW_QUERY: &str =
    r#"evidence_query = "{device=\"{device}\",peer_address=\"{peer}\"} != \"license\"""#;

const METRICS: [&str; 6] = [
    "bgp_admin_state",
    "bgp_oper_state",
    "bgp_received_routes",
    "bgp_sent_routes",
    "bgp_active_routes",
    "bgp_suppressed_routes",
];
const VALUES: [&str; 6] = ["1", "5", "0", "10", "10", "0"];
const LABELS: &str = "%7Bdevice%3D%22srl1%22%2Cpeer_address%3D%2210.1.99.2%22%2Cafi_safi_name%3D%22ipv4-unicast%22%2Cname%3D%22default%22%7D";

const NO_STREAMS: &str = r#"{"status":"success","data":{"resultType":"streams","result":[]}}"#;
const NO_SERIES: &str = r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#;
const NO_EDGES: &str = r#"{"data":{"WorkshopDevice":{"edges":[]}}}"#;
const INTENT: &str = r#"{"data":{"WorkshopDevice":{"edges":[{"node":{"name":{"value":"srl1"},"maintenance":{"value":false},"site_name":{"value":"lab"},"role":{"value":"edge"},"bgp_sessions":{"edges":[{"node":{"peer_address":{"value":"10.1.99.2"},"afi_safi":{"value":"ipv4-unicast"},"expected_state":{"value":"established"},"remote_as":{"value":65102},"reason":{"value":"ip-mismatch-demo"}}}]}}}]}}}"#;

const NEWER: &str = "1793960401500000000";
const OLDER: &str = "1793960148000000000";
const RCA_NEWER: &str = r#"{"timestamp":"2026-11-06T10:20:01.500Z","message":"AI RCA: Check the interface first.","labels":{"device":"srl1"}}"#;
const RCA_OLDER: &str = r#"{"timestamp":"2026-11-06T10:15:48.201Z","severity":"info","message":"AI RCA:\n\n## Most likely cause\nThe peer 10.1.99.2 stopped answering.","labels":{"device":"srl1","peer_address":"10.1.99.2"}}"#;
const LOG_NEWER: &str =
    "2026-11-06T10:20:00Z srl1 bgp neighbor 10.1.99.2 session state changed to active";
const LOG_OLDER: &str =
    "2026-11-06T10:15:48Z srl1 bgp neighbor 10.1.99.2 session state changed to idle";

const RCA_HELP: &str = "Print the most recent AI RCA narratives from Loki

Usage: nobs <workshop> rca [OPTIONS] [DEVICE] [PEER]

Arguments:
  [DEVICE]  Device name (e.g. srl1). Optional — defaults to any device.
  [PEER]    Peer IP (e.g. 10.1.99.2). Optional — defaults to any peer.

Options:
      --last <N>        Number of most-recent records to show. [default: 1]
      --minutes <N>     How far back to look. [default: 60]
      --loki-url <URL>  Address of Loki; nobs looks at --loki-url, then LOKI_URL, then http://localhost:3001
  -h, --help            Print help
";
const EVIDENCE_HELP: &str = "Print the source-of-truth gate, BGP metrics snapshot, recent log lines, and a policy hint for one device and peer

Usage: nobs <workshop> evidence [OPTIONS] <DEVICE> <PEER>

Arguments:
  <DEVICE>  Device name (e.g. srl1).
  <PEER>    Peer IP address (e.g. 10.1.99.2).

Options:
      --afi-safi <NAME>     Address family of the BGP session [default: ipv4-unicast]
      --instance <NAME>     Name of the network instance of the BGP session [default: default]
      --log-minutes <N>     How many minutes back to look for log lines [default: 10]
      --log-limit <N>       How many log lines to show at most [default: 20]
      --prom-url <URL>      Address of Prometheus; nobs looks at --prom-url, then PROMETHEUS_URL, then http://localhost:9090
      --loki-url <URL>      Address of Loki; nobs looks at --loki-url, then LOKI_URL, then http://localhost:3001
      --infrahub-url <URL>  Address of Infrahub; nobs looks at --infrahub-url, then INFRAHUB_ADDRESS, then http://localhost:8000
      --token <TOKEN>       Token for Infrahub; nobs looks at --token, then INFRAHUB_API_TOKEN
  -h, --help                Print help
";

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str, env: &str) -> Self {
        let root =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-flow-views-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture(), &root.join("nobs-t-one"));
        let copy = Self(root.canonicalize().unwrap());
        let settings = fs::read_to_string(copy.dir().join("nobs.toml")).unwrap();
        assert!(settings.contains(OLD_QUERY), "{settings}");
        fs::write(
            copy.dir().join("nobs.toml"),
            settings.replace(OLD_QUERY, NEW_QUERY),
        )
        .unwrap();
        fs::write(copy.dir().join(".env"), env).unwrap();
        copy
    }

    fn dir(&self) -> PathBuf {
        self.0.join("nobs-t-one")
    }

    fn shown(&self) -> String {
        self.dir().display().to_string()
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Stack {
    copy: Copy,
    loki: Stub,
    prometheus: Stub,
    infrahub: Stub,
}

impl Stack {
    fn new(name: &str, loki: Stub, prometheus: Stub, infrahub: Stub) -> Self {
        let env = env(loki.url(), prometheus.url(), infrahub.url(), true);
        Self {
            copy: Copy::new(name, &env),
            loki,
            prometheus,
            infrahub,
        }
    }

    fn healthy(name: &str) -> Self {
        Self::new(
            name,
            loki(&loki_reply(&[(OLDER, LOG_OLDER), (NEWER, LOG_NEWER)])),
            prometheus(&VALUES.map(series)),
            infrahub(200, INTENT),
        )
    }

    fn run(&self, args: &[&str]) -> Output {
        run(&self.copy, args)
    }

    fn requests(&self) -> usize {
        self.loki.requests().len()
            + self.prometheus.requests().len()
            + self.infrahub.requests().len()
    }
}

fn env(loki: &str, prometheus: &str, infrahub: &str, token: bool) -> String {
    let mut text =
        format!("LOKI_URL={loki}\nPROMETHEUS_URL={prometheus}\nINFRAHUB_ADDRESS={infrahub}\n");
    if token {
        text.push_str(&format!("INFRAHUB_API_TOKEN={TOKEN}\n"));
    }
    text
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

fn json_text(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn loki_reply(lines: &[(&str, &str)]) -> String {
    let values: Vec<String> = lines
        .iter()
        .map(|(nanoseconds, text)| format!(r#"["{nanoseconds}","{}"]"#, json_text(text)))
        .collect();
    format!(
        r#"{{"status":"success","data":{{"resultType":"streams","result":[{{"stream":{{}},"values":[{}]}}]}}}}"#,
        values.join(",")
    )
}

fn series(value: &str) -> String {
    format!(
        r#"{{"status":"success","data":{{"resultType":"vector","result":[{{"metric":{{}},"value":[1793960400,"{value}"]}}]}}}}"#
    )
}

fn loki(body: &str) -> Stub {
    Stub::start(&[("GET", LOKI_ROUTE, 200, body)])
}

fn prometheus(bodies: &[String; 6]) -> Stub {
    let paths: Vec<String> = METRICS
        .iter()
        .map(|name| format!("/api/v1/query?query={name}*"))
        .collect();
    let routes: Vec<(&str, &str, u16, &str)> = paths
        .iter()
        .zip(bodies)
        .map(|(path, body)| ("GET", path.as_str(), 200, body.as_str()))
        .collect();
    Stub::start(&routes)
}

fn infrahub(status: u16, body: &str) -> Stub {
    Stub::start(&[("POST", GRAPHQL, status, body)])
}

fn run(copy: &Copy, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command.arg("nobs-t-one").args(args).current_dir(copy.dir());
    for name in [
        "LOKI_URL",
        "PROMETHEUS_URL",
        "INFRAHUB_ADDRESS",
        "INFRAHUB_API_TOKEN",
        "NOBS_DIR",
    ] {
        command.env_remove(name);
    }
    command.output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn blocks(output: &Output) -> Vec<String> {
    let text = stdout(output);
    let body = text
        .strip_prefix('\n')
        .and_then(|text| text.strip_suffix('\n'))
        .unwrap_or_else(|| panic!("a blank line first and a line break last: {text:?}"));
    body.split("\n\n").map(str::to_string).collect()
}

fn parameters(seen: &Seen) -> Vec<&str> {
    let (path, query) = seen.path.split_once('?').unwrap();
    assert_eq!(path, LOKI_PATH);
    query.split('&').collect()
}

fn window_minutes(parameters: &[&str]) -> i64 {
    let number = |name: &str| -> i64 {
        parameters
            .iter()
            .find_map(|parameter| parameter.strip_prefix(name))
            .unwrap()
            .parse()
            .unwrap()
    };
    (number("end=") - number("start=")) / NANOSECONDS_PER_MINUTE
}

fn help(command: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["nobs-t-one", command, "--help"])
        .current_dir(fixture())
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    stdout(&output)
}

#[test]
fn the_rca_help_page_holds_the_arguments_and_the_flags() {
    assert_eq!(help("rca"), RCA_HELP);
}

#[test]
fn the_evidence_help_page_holds_the_arguments_and_the_flags() {
    assert_eq!(help("evidence"), EVIDENCE_HELP);
}

#[test]
fn rca_with_no_arguments_asks_loki_for_any_device_and_peer_and_says_when_it_finds_nothing() {
    let loki = loki(NO_STREAMS);
    let copy = Copy::new("rca-none", &format!("LOKI_URL={}\n", loki.url()));

    let output = run(&copy, &["rca"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!(
            "No AI RCA records found in the last 60m for any device/peer. If you haven't enabled it yet, set ENABLE_AI_RCA=true in {}/.env and re-run `nobs nobs-t-one up`.\n",
            copy.shown()
        )
    );
    assert_eq!(stderr(&output), "");
    let requests = loki.requests();
    assert_eq!(requests.len(), 1);
    let parameters = parameters(&requests[0]);
    assert_eq!(
        parameters[0],
        "query=%7Bsource%3D%22prefect%22%2C%20ai_rca%3D%22true%22%7D"
    );
    assert!(parameters.contains(&"limit=1"), "{parameters:?}");
    assert!(parameters.contains(&"direction=BACKWARD"), "{parameters:?}");
    assert_eq!(window_minutes(&parameters), 60);
}

#[test]
fn rca_takes_the_loki_address_from_the_flag_before_the_env_file() {
    let loki = loki(NO_STREAMS);
    let copy = Copy::new("rca-flag", &format!("LOKI_URL={}\n", closed_port_url()));

    let output = run(&copy, &["rca", "--loki-url", loki.url()]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("No AI RCA records found"));
    assert_eq!(loki.requests().len(), 1);
}

#[test]
fn rca_names_the_device_or_the_peer_that_was_given_in_the_hint() {
    let loki = loki(NO_STREAMS);
    let copy = Copy::new("rca-scope", &format!("LOKI_URL={}\n", loki.url()));

    let device = stdout(&run(&copy, &["rca", "srl1"]));
    let peer = stdout(&run(&copy, &["rca", "", "10.1.99.2"]));
    let both = stdout(&run(&copy, &["rca", "srl1", "10.1.99.2"]));

    assert!(device.contains("last 60m for device=srl1. If"), "{device}");
    assert!(peer.contains("last 60m for peer=10.1.99.2. If"), "{peer}");
    assert!(
        both.contains("last 60m for device=srl1 peer=10.1.99.2. If"),
        "{both}"
    );
    let requests = loki.requests();
    assert!(
        parameters(&requests[0])[0].ends_with("%2C%20device%3D%22srl1%22%7D"),
        "{:?}",
        requests[0].path
    );
    assert!(
        parameters(&requests[1])[0].ends_with("%2C%20peer_address%3D%2210.1.99.2%22%7D"),
        "{:?}",
        requests[1].path
    );
    assert!(
        parameters(&requests[2])[0]
            .ends_with("%2C%20device%3D%22srl1%22%2C%20peer_address%3D%2210.1.99.2%22%7D"),
        "{:?}",
        requests[2].path
    );
}

#[test]
fn rca_prints_the_records_newest_first_without_the_prefix() {
    let loki = loki(&loki_reply(&[(OLDER, RCA_OLDER), (NEWER, RCA_NEWER)]));
    let copy = Copy::new("rca-records", &format!("LOKI_URL={}\n", loki.url()));

    let output = run(
        &copy,
        &["rca", "srl1", "10.1.99.2", "--last", "2", "--minutes", "5"],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "\n2026-11-06T10:20:01.500Z  device=srl1  peer=?\n Check the interface first.\n\n2026-11-06T10:15:48.201Z  device=srl1  peer=10.1.99.2\n## Most likely cause\nThe peer 10.1.99.2 stopped answering.\n\n"
    );
    let requests = loki.requests();
    let parameters = parameters(&requests[0]);
    assert!(parameters.contains(&"limit=2"), "{parameters:?}");
    assert_eq!(window_minutes(&parameters), 5);
    assert!(
        parameters[0].contains("device%3D%22srl1%22") && parameters[0].contains("10.1.99.2"),
        "{parameters:?}"
    );
}

#[test]
fn rca_says_which_record_it_skips_when_a_line_is_not_json() {
    let loki = loki(&loki_reply(&[
        (OLDER, "not json at all"),
        (NEWER, RCA_NEWER),
    ]));
    let copy = Copy::new("rca-skip", &format!("LOKI_URL={}\n", loki.url()));

    let output = run(
        &copy,
        &["rca", "srl1", "10.1.99.2", "--last", "2", "--minutes", "5"],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "\n2026-11-06T10:20:01.500Z  device=srl1  peer=?\n Check the interface first.\n\n(skipping unparseable record #2)\n"
    );
}

#[test]
fn rca_prints_the_error_and_exits_1_when_loki_answers_500() {
    let loki = Stub::start(&[("GET", LOKI_ROUTE, 500, "boom")]);
    let copy = Copy::new("rca-500", &format!("LOKI_URL={}\n", loki.url()));

    let output = run(&copy, &["rca"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: Failed to query Loki at {}: Loki answered 500 on {LOKI_PATH}: boom\n",
            loki.url()
        )
    );
}

#[test]
fn rca_gives_the_reason_without_a_second_cannot_reach_when_loki_is_not_listening() {
    let url = closed_port_url();
    let copy = Copy::new("rca-closed", &format!("LOKI_URL={url}\n"));

    let output = run(&copy, &["rca"]);

    let message = stderr(&output);
    let prefix = format!("nobs: Failed to query Loki at {url}: ");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    let reason = message
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("{message}"));
    assert!(!reason.trim().is_empty(), "{message}");
    assert!(!reason.contains("cannot reach"), "{message}");
}

#[test]
fn evidence_prints_the_four_blocks() {
    let stack = Stack::healthy("evidence-blocks");

    let output = stack.run(&["evidence", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let expected = [
        "",
        "Source of truth (Infrahub)",
        "device          srl1   site=lab  role=edge",
        "maintenance     false",
        "intended peer   yes",
        "expected state  established",
        "reason          ip-mismatch-demo",
        "remote_as       65102",
        "",
        "BGP metrics snapshot (Prometheus)",
        "Metric             Value  Decoded",
        "admin_state            1  enable",
        "oper_state             5  active",
        "received_routes        0  —",
        "sent_routes           10  —",
        "active_routes         10  —",
        "suppressed_routes      0  —",
        "",
        "Loki - last 2 relevant line(s)",
        LOG_NEWER,
        LOG_OLDER,
        "",
        "Policy hint",
        "decision: proceed",
        "reason  : SoT expects peer up, but metrics show mismatch",
        "",
    ]
    .join("\n");
    assert_eq!(stdout(&output), expected);
    assert_eq!(stderr(&output), "");
}

#[test]
fn evidence_asks_infrahub_once_then_prometheus_six_times_in_order_then_loki_once() {
    let metric_paths: Vec<String> = METRICS
        .iter()
        .map(|name| format!("/api/v1/query?query={name}*"))
        .collect();
    let metric_bodies = VALUES.map(series);
    let loki_body = loki_reply(&[(OLDER, LOG_OLDER), (NEWER, LOG_NEWER)]);
    let mut routes = vec![
        ("POST", GRAPHQL, 200, INTENT),
        ("GET", LOKI_ROUTE, 200, loki_body.as_str()),
    ];
    routes.extend(
        metric_paths
            .iter()
            .zip(&metric_bodies)
            .map(|(path, body)| ("GET", path.as_str(), 200, body.as_str())),
    );
    let everything = Stub::start(&routes);
    let url = everything.url();
    let copy = Copy::new("evidence-requests", &env(url, url, url, true));

    let output = run(&copy, &["evidence", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let requests = everything.requests();
    let order: Vec<String> = requests
        .iter()
        .map(|seen| match seen.path.starts_with(LOKI_PATH) {
            true => format!("{} {LOKI_PATH}", seen.method),
            false => format!("{} {}", seen.method, seen.path),
        })
        .collect();
    let mut expected = vec![format!("POST {GRAPHQL}")];
    expected.extend(
        METRICS
            .iter()
            .map(|name| format!("GET /api/v1/query?query={name}{LABELS}")),
    );
    expected.push(format!("GET {LOKI_PATH}"));
    assert_eq!(order, expected);
    assert_eq!(requests[0].header("X-INFRAHUB-KEY"), Some(TOKEN));
    let body = String::from_utf8_lossy(&requests[0].body).into_owned();
    assert!(body.contains(r#""variables":{"name":"srl1"}"#), "{body}");
    let parameters = parameters(&requests[7]);
    assert_eq!(
        parameters[0],
        "query=%7Bdevice%3D%22srl1%22%2Cpeer_address%3D%2210.1.99.2%22%7D%20!%3D%20%22license%22"
    );
    assert!(parameters.contains(&"limit=20"), "{parameters:?}");
    assert!(parameters.contains(&"direction=BACKWARD"), "{parameters:?}");
    assert_eq!(window_minutes(&parameters), 10);
}

#[test]
fn evidence_puts_the_instance_the_log_window_and_the_log_limit_into_its_requests() {
    let stack = Stack::healthy("evidence-options");

    let output = stack.run(&[
        "evidence",
        "srl1",
        "10.1.99.2",
        "--instance",
        "edge",
        "--log-minutes",
        "3",
        "--log-limit",
        "5",
    ]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    for seen in stack.prometheus.requests() {
        assert!(
            seen.path.ends_with("%2Cname%3D%22edge%22%7D"),
            "{}",
            seen.path
        );
    }
    let loki = stack.loki.requests();
    let parameters = parameters(&loki[0]);
    assert!(parameters.contains(&"limit=5"), "{parameters:?}");
    assert_eq!(window_minutes(&parameters), 3);
}

#[test]
fn evidence_takes_the_addresses_and_the_token_from_the_flags_before_the_env_file() {
    let stack = Stack::healthy("evidence-flags");
    let closed = closed_port_url();
    let copy = Copy::new(
        "evidence-flags-copy",
        &env(&closed, &closed, &closed, false),
    );
    let flagged = |flag: &str, value: &str| [flag.to_string(), value.to_string()];
    let mut args = vec![
        "evidence".to_string(),
        "srl1".to_string(),
        "10.1.99.2".to_string(),
    ];
    for pair in [
        flagged("--prom-url", stack.prometheus.url()),
        flagged("--loki-url", stack.loki.url()),
        flagged("--infrahub-url", stack.infrahub.url()),
        flagged("--token", "flag-token"),
    ] {
        args.extend(pair);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    let output = run(&copy, &args);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(blocks(&output).len(), 4);
    assert_eq!(stack.prometheus.requests().len(), 6);
    assert_eq!(stack.loki.requests().len(), 1);
    let infrahub = stack.infrahub.requests();
    assert_eq!(infrahub.len(), 1);
    assert_eq!(infrahub[0].header("X-INFRAHUB-KEY"), Some("flag-token"));
}

#[test]
fn evidence_prints_the_rewrite_line_first_and_then_asks_for_the_token() {
    let loki = loki(NO_STREAMS);
    let prometheus = Stub::start(&[("GET", PROMETHEUS_ROUTE, 200, NO_SERIES)]);
    let env = env(
        loki.url(),
        prometheus.url(),
        "http://infrahub-server:8000",
        false,
    );
    let copy = Copy::new("evidence-rewrite", &env);

    let output = run(&copy, &["evidence", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), REWRITTEN);
    assert_eq!(stderr(&output), TOKEN_REQUIRED);
    assert!(loki.requests().is_empty());
    assert!(prometheus.requests().is_empty());
}

#[test]
fn evidence_without_a_token_prints_the_required_line_and_sends_nothing() {
    let stack = Stack::healthy("evidence-no-token");
    let env = env(
        stack.loki.url(),
        stack.prometheus.url(),
        stack.infrahub.url(),
        false,
    );
    let copy = Copy::new("evidence-no-token-copy", &env);

    let output = run(&copy, &["evidence", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), TOKEN_REQUIRED);
    assert_eq!(stack.requests(), 0);
}

#[test]
fn evidence_says_so_when_infrahub_has_no_such_device_and_stops() {
    let stack = Stack::new(
        "evidence-no-device",
        loki(NO_STREAMS),
        prometheus(&VALUES.map(series)),
        infrahub(200, NO_EDGES),
    );

    let output = stack.run(&["evidence", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let blocks = blocks(&output);
    assert_eq!(blocks[0], "Source of truth\ndevice not found in Infrahub.");
    assert_eq!(
        blocks[3],
        "Policy hint\ndecision: stop\nreason  : device not found in Infrahub"
    );
}

#[test]
fn evidence_shows_the_error_and_exits_0_when_the_infrahub_query_fails() {
    let stack = Stack::new(
        "evidence-infrahub-500",
        loki(NO_STREAMS),
        prometheus(&VALUES.map(series)),
        infrahub(500, "boom"),
    );

    let output = stack.run(&["evidence", "srl1", "10.1.99.2"]);

    let failed = "Infrahub query failed: Infrahub answered 500 on /graphql/main: boom";
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let blocks = blocks(&output);
    assert_eq!(blocks[0], format!("Source of truth\n{failed}"));
    assert_eq!(
        blocks[3],
        format!("Policy hint\ndecision: stop\nreason  : {failed}")
    );
}

#[test]
fn evidence_prints_minus_one_for_a_state_and_zero_for_a_counter_when_prometheus_has_no_series() {
    let stack = Stack::new(
        "evidence-no-series",
        loki(NO_STREAMS),
        prometheus(&[(); 6].map(|_| NO_SERIES.to_string())),
        infrahub(200, INTENT),
    );

    let output = stack.run(&["evidence", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let blocks = blocks(&output);
    assert_eq!(
        blocks[1],
        [
            "BGP metrics snapshot (Prometheus)",
            "Metric             Value  Decoded",
            "admin_state           -1  —",
            "oper_state            -1  —",
            "received_routes        0  —",
            "sent_routes            0  —",
            "active_routes          0  —",
            "suppressed_routes      0  —",
        ]
        .join("\n")
    );
    assert_eq!(
        blocks[3],
        "Policy hint\ndecision: proceed\nreason  : SoT expects peer up, but metrics show mismatch"
    );
}

#[test]
fn evidence_counts_a_failing_prometheus_as_missing_values_and_exits_0() {
    let failing = Stub::start(&[("GET", PROMETHEUS_ROUTE, 500, "boom")]);
    let stack = Stack::new(
        "evidence-prometheus-500",
        loki(NO_STREAMS),
        failing,
        infrahub(200, INTENT),
    );

    let output = stack.run(&["evidence", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let blocks = blocks(&output);
    assert!(
        blocks[1].contains("\nadmin_state           -1  —\n"),
        "{}",
        blocks[1]
    );
    assert!(
        blocks[1].ends_with("suppressed_routes      0  —"),
        "{}",
        blocks[1]
    );
}

#[test]
fn evidence_marks_the_peer_as_not_intended_when_the_afi_safi_does_not_match() {
    let stack = Stack::healthy("evidence-afi-safi");

    let output = stack.run(&[
        "evidence",
        "srl1",
        "10.1.99.2",
        "--afi-safi",
        "ipv6-unicast",
    ]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let blocks = blocks(&output);
    assert_eq!(
        blocks[0],
        [
            "Source of truth (Infrahub)",
            "device          srl1   site=lab  role=edge",
            "maintenance     false",
            "intended peer   no",
        ]
        .join("\n")
    );
    assert_eq!(
        blocks[3],
        "Policy hint\ndecision: skip\nreason  : peer not intended in SoT"
    );
    for seen in stack.prometheus.requests() {
        assert!(
            seen.path.contains("afi_safi_name%3D%22ipv6-unicast%22"),
            "{}",
            seen.path
        );
    }
}

#[test]
fn evidence_says_there_are_no_log_lines_and_still_exits_0_when_loki_fails() {
    let failing = Stub::start(&[("GET", LOKI_ROUTE, 500, "boom")]);
    let stack = Stack::new(
        "evidence-loki-500",
        failing,
        prometheus(&VALUES.map(series)),
        infrahub(200, INTENT),
    );

    let output = stack.run(&["evidence", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        blocks(&output)[2],
        "Loki\nNo relevant log lines in the window."
    );
}
