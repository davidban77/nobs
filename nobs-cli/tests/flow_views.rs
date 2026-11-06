use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

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
const CYCLE_HELP: &str = "Capture the workflow's full cycle state for one device and peer; optionally re-drive it

Usage: nobs <workshop> cycle [OPTIONS] <DEVICE> <PEER>

Arguments:
  <DEVICE>  Device name (e.g. srl1).
  <PEER>    Peer IP (e.g. 10.1.99.2).

Options:
      --trigger            Post a fresh alert payload through the webhook before rendering.
      --status <STATUS>    Alert status to post with --trigger: 'firing' (default) or 'resolved'. [possible values: firing, resolved]
      --minutes <N>        Lookback window for recent panels. [default: 30]
      --am-url <URL>       Address of Alertmanager; nobs looks at --am-url, then ALERTMANAGER_URL, then http://localhost:9093
      --loki-url <URL>     Address of Loki; nobs looks at --loki-url, then LOKI_URL, then http://localhost:3001
      --prefect-url <URL>  Address of Prefect; nobs looks at --prefect-url, then PREFECT_API_URL, then http://localhost:4200
      --webhook-url <URL>  Address of the webhook; nobs looks at --webhook-url, then WEBHOOK_URL, then http://localhost:9997/v1/api/webhook
  -h, --help               Print help
";
const PAGE_HELP: &str = "Render a fake on-call page from a live alert

Usage: nobs <workshop> page [OPTIONS]

Options:
  -a, --alert <ALERT>    Alertname to filter by; the default is BgpSessionNotUp
  -d, --device <DEVICE>  Filter to alerts on this device.
  -p, --peer <PEER>      Filter to alerts on this peer_address.
      --now <HH:MM>      Override the wall-clock printed in the page (HH:MM). Default: real time.
      --am-url <URL>     Address of Alertmanager; nobs looks at --am-url, then ALERTMANAGER_URL, then http://localhost:9093
  -h, --help             Print help
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
    run_with(copy, args, &[])
}

fn run_with(copy: &Copy, args: &[&str], variables: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command.arg("nobs-t-one").args(args).current_dir(copy.dir());
    for name in [
        "LOKI_URL",
        "PROMETHEUS_URL",
        "ALERTMANAGER_URL",
        "PREFECT_API_URL",
        "WEBHOOK_URL",
        "INFRAHUB_ADDRESS",
        "INFRAHUB_API_TOKEN",
        "NOBS_DIR",
    ] {
        command.env_remove(name);
    }
    for (name, value) in variables {
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
fn the_cycle_help_page_holds_the_arguments_and_the_flags() {
    assert_eq!(help("cycle"), CYCLE_HELP);
}

#[test]
fn the_page_help_page_holds_the_flags() {
    assert_eq!(help("page"), PAGE_HELP);
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

const ALERTS_ROUTE: &str = "/api/v2/alerts?active=true&silenced=true&inhibited=true";
const SILENCES_ROUTE: &str = "/api/v2/silences";
const FLOW_RUNS_ROUTE: &str = "/api/flow_runs/filter";
const TASK_RUNS_ROUTE: &str = "/api/task_runs/filter";
const WEBHOOK_ROUTE: &str = "/v1/api/webhook";
const SILENCED_BY: &str = "3f2a9c1e-7b4d-4e1a-9c2b-0d5e6f7a8b9c";
const TASKS: &str = r#"[{"id":"t0","flow_run_id":"r0"},{"id":"t1","flow_run_id":"q1"},{"id":"t2","flow_run_id":"q1"},{"id":"t3","flow_run_id":"q1"}]"#;
const DECISION_QUERY: &str = r#"{source="prefect", workflow="nobs_t_one_quarantine_bgp", device="srl1", peer_address="10.1.99.2", decision=~".+"} | json"#;
const DECISION_MESSAGE: &str = "SoT expects peer up, but metrics show mismatch";
const QUARANTINE: &str = "quarantine_bgp | srl1:10.1.99.2";
const FLOW_RUNS_HEADER: &str = "Started   State      Flow";
const HINT: [&str; 3] = [
    "",
    "  To drive a fresh cycle (bypassing Alertmanager's repeat_interval):",
    "    nobs nobs-t-one cycle srl1 10.1.99.2 --trigger",
];
const POSTED: &str = "posted firing payload for srl1 → 10.1.99.2; waiting for new flow run …";
const BOOM_WEBHOOK: &str = "nobs: could not POST to webhook: the webhook answered 500: boom\n";
const NO_MATCH: &str = "nobs: no firing alert matched the filters. Run `nobs nobs-t-one alerts` to see what's currently firing.\n";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86400) + 719_468;
    let rest = seconds.rem_euclid(86400);
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn clock_of(seconds: i64) -> String {
    rfc3339(seconds)[11..19].to_string()
}

fn hours_and_minutes(seconds: i64) -> String {
    let rest = seconds.rem_euclid(86400);
    format!("{:02}:{:02}", rest / 3600, rest % 3600 / 60)
}

fn object(pairs: &[(&str, &str)]) -> String {
    let fields: Vec<String> = pairs
        .iter()
        .map(|(name, value)| format!(r#""{name}":"{value}""#))
        .collect();
    format!("{{{}}}", fields.join(","))
}

fn bgp(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    let mut labels = vec![
        ("alertname", "BgpSessionNotUp"),
        ("device", "srl1"),
        ("peer_address", "10.1.99.2"),
    ];
    labels.extend_from_slice(extra);
    labels
}

fn alert_json(
    labels: &[(&str, &str)],
    annotations: &[(&str, &str)],
    state: &str,
    silenced_by: &[&str],
    starts_at: &str,
) -> String {
    let silenced: Vec<String> = silenced_by.iter().map(|id| format!(r#""{id}""#)).collect();
    format!(
        r#"{{"labels":{},"annotations":{},"status":{{"state":"{state}","silencedBy":[{}],"inhibitedBy":[]}},"startsAt":"{starts_at}"}}"#,
        object(labels),
        object(annotations),
        silenced.join(",")
    )
}

fn silence_json(id: &str, state: &str, starts: i64, ends: i64, peer: &str) -> String {
    format!(
        r#"{{"id":"{id}","status":{{"state":"{state}"}},"startsAt":"{}","endsAt":"{}","createdBy":"prefect","matchers":[{{"name":"alertname","value":"BgpSessionNotUp","isRegex":false}},{{"name":"device","value":"srl1","isRegex":false}},{{"name":"peer_address","value":"{peer}","isRegex":false}}]}}"#,
        rfc3339(starts),
        rfc3339(ends)
    )
}

fn run_json(id: &str, name: &str, started: &str, parent: Option<&str>) -> String {
    let parent = parent.map_or("null".to_string(), |task| format!(r#""{task}""#));
    format!(
        r#"{{"id":"{id}","name":"{name}","parameters":{{"device":"srl1","peer_address":"10.1.99.2"}},"start_time":"{started}","state":{{"type":"COMPLETED"}},"parent_task_run_id":{parent}}}"#
    )
}

fn packt_runs(t: i64, newest: &[String]) -> String {
    let at = |offset: i64| rfc3339(t - offset);
    let receiver = format!(
        r#"{{"id":"r0","name":"alert_receiver | BgpSessionNotUp:firing","parameters":{{"alertname":"BgpSessionNotUp","status":"firing","alert_group":{{}}}},"start_time":"{}","state":{{"type":"COMPLETED"}},"parent_task_run_id":null}}"#,
        at(261)
    );
    let mut runs = newest.to_vec();
    runs.extend([
        run_json("a1", "action | srl1:10.1.99.2", &at(255), Some("t3")),
        run_json("p1", "policy | srl1:10.1.99.2", &at(256), Some("t2")),
        run_json("e1", "evidence | srl1:10.1.99.2", &at(259), Some("t1")),
        run_json(
            "q1",
            QUARANTINE,
            &at(260).replace(".000Z", ".512345Z"),
            Some("t0"),
        ),
        receiver,
    ]);
    format!("[{}]", runs.join(","))
}

fn decision_reply(t: i64) -> String {
    let line = format!(
        r#"{{"timestamp":"{}","severity":"info","message":"{DECISION_MESSAGE}","labels":{{"decision":"proceed","device":"srl1","peer_address":"10.1.99.2"}}}}"#,
        rfc3339(t - 252).replace(".000Z", ".201Z")
    );
    loki_reply(&[(&format!("{}000000000", t - 252), &line)])
}

struct Responses {
    alerts: (u16, String),
    silences: (u16, String),
    flow_runs: Vec<(u16, String)>,
    task_runs: (u16, String),
    loki: (u16, String),
    webhook: (u16, String),
}

impl Responses {
    fn packt(t: i64) -> Self {
        let alert = alert_json(
            &bgp(&[("severity", "warning")]),
            &[],
            "suppressed",
            &[SILENCED_BY],
            &rfc3339(t - 269),
        );
        Self {
            alerts: (200, format!("[{alert}]")),
            silences: (
                200,
                format!(
                    "[{}]",
                    silence_json(SILENCED_BY, "active", t - 240, t + 960, "10.1.99.2")
                ),
            ),
            flow_runs: vec![(200, packt_runs(t, &[]))],
            task_runs: (200, TASKS.to_string()),
            loki: (200, decision_reply(t)),
            webhook: (204, String::new()),
        }
    }

    fn nothing() -> Self {
        Self {
            alerts: (200, "[]".to_string()),
            silences: (200, "[]".to_string()),
            flow_runs: vec![(200, "[]".to_string())],
            task_runs: (200, "[]".to_string()),
            loki: (200, NO_STREAMS.to_string()),
            webhook: (204, String::new()),
        }
    }

    fn failing() -> Self {
        let boom = || (500, "boom".to_string());
        Self {
            alerts: boom(),
            silences: boom(),
            flow_runs: vec![boom()],
            task_runs: boom(),
            loki: boom(),
            webhook: boom(),
        }
    }

    fn serve(&self) -> Stub {
        let mut routes: Vec<(&str, &str, u16, &str)> = vec![
            ("GET", ALERTS_ROUTE, self.alerts.0, &self.alerts.1),
            ("GET", SILENCES_ROUTE, self.silences.0, &self.silences.1),
            ("POST", TASK_RUNS_ROUTE, self.task_runs.0, &self.task_runs.1),
            ("GET", LOKI_ROUTE, self.loki.0, &self.loki.1),
            ("POST", WEBHOOK_ROUTE, self.webhook.0, &self.webhook.1),
        ];
        routes.extend(
            self.flow_runs
                .iter()
                .map(|(status, body)| ("POST", FLOW_RUNS_ROUTE, *status, body.as_str())),
        );
        Stub::start(&routes)
    }
}

fn cycle_env(url: &str) -> String {
    format!(
        "ALERTMANAGER_URL={url}\nLOKI_URL={url}\nPREFECT_API_URL={url}/api\nWEBHOOK_URL={url}{WEBHOOK_ROUTE}\n"
    )
}

fn order(requests: &[Seen]) -> Vec<String> {
    requests
        .iter()
        .map(|seen| match seen.path.starts_with(LOKI_PATH) {
            true => format!("{} {LOKI_PATH}", seen.method),
            false => format!("{} {}", seen.method, seen.path),
        })
        .collect()
}

fn decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut plain = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            plain.push(u8::from_str_radix(&text[index + 1..index + 3], 16).unwrap());
            index += 3;
        } else {
            plain.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(plain).unwrap()
}

fn panel(output: &Output, title: &str) -> String {
    let text = stdout(output);
    let from = text
        .find(&format!("\n{title}\n"))
        .unwrap_or_else(|| panic!("no panel {title}: {text}"))
        + 1;
    text[from..]
        .split("\n\n")
        .next()
        .unwrap()
        .trim_end_matches('\n')
        .to_string()
}

fn cells(line: &str) -> Vec<&str> {
    line.split("  ")
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .collect()
}

fn packt_output(t: i64, age: &str, remaining: &str) -> String {
    let at = |offset: i64| clock_of(t + offset);
    let mut lines = vec![
        String::new(),
        "Alert".to_string(),
        format!("BgpSessionNotUp · warning · state=suppressed · age {age}"),
        "silencedBy: 3f2a9c1e-7b4…".to_string(),
        String::new(),
        "Silences (this peer)".to_string(),
        "ID             State   Starts    Ends      Remaining".to_string(),
        format!(
            "3f2a9c1e-7b4…  active  {}  {}  {remaining}",
            at(-240),
            at(960)
        ),
        String::new(),
        "Prefect flow runs (last 30m)".to_string(),
        FLOW_RUNS_HEADER.to_string(),
        format!("{}  COMPLETED  {QUARANTINE}", at(-260)),
        format!("{}  COMPLETED    ├─ evidence", at(-259)),
        format!("{}  COMPLETED    ├─ policy", at(-256)),
        format!("{}  COMPLETED    └─ action", at(-255)),
        String::new(),
        "Most recent decision".to_string(),
        format!("{}  decision=proceed", at(-252)),
        format!("reason: {DECISION_MESSAGE}"),
    ];
    lines.extend(HINT.map(String::from));
    lines.push(String::new());
    lines.join("\n")
}

#[test]
fn cycle_asks_in_order_and_prints_the_four_panels_then_the_hint() {
    let t = now();
    let stub = Responses::packt(t).serve();
    let copy = Copy::new("cycle-panels", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let out = stdout(&output);
    let expected = [("4m29s", "16m0s"), ("4m30s", "15m59s")]
        .map(|(age, remaining)| packt_output(t, age, remaining));
    assert!(expected.contains(&out), "{out}");
    let requests = stub.requests();
    assert_eq!(
        order(&requests),
        [
            format!("GET {ALERTS_ROUTE}"),
            format!("GET {SILENCES_ROUTE}"),
            format!("POST {FLOW_RUNS_ROUTE}"),
            format!("POST {TASK_RUNS_ROUTE}"),
            format!("GET {LOKI_PATH}"),
        ]
    );
    assert_eq!(
        String::from_utf8_lossy(&requests[3].body),
        r#"{"task_runs":{"id":{"any_":["t0","t1","t2","t3"]}},"limit":4}"#
    );
    let parameters = parameters(&requests[4]);
    assert_eq!(
        decoded(parameters[0].strip_prefix("query=").unwrap()),
        DECISION_QUERY
    );
    assert!(parameters.contains(&"limit=1"), "{parameters:?}");
    assert_eq!(window_minutes(&parameters), 30);
}

#[test]
fn cycle_shows_each_error_under_its_title_and_still_exits_0() {
    let stub = Responses::failing().serve();
    let url = stub.url();
    let copy = Copy::new("cycle-errors", &cycle_env(url));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let mut lines = vec![
        String::new(),
        "Alert".to_string(),
        format!("Alertmanager query failed: Alertmanager at {url} answered 500"),
        String::new(),
        "Silences (this peer)".to_string(),
        format!("silences query failed: Alertmanager at {url} answered 500"),
        String::new(),
        "Prefect flow runs (last 30m)".to_string(),
        format!("Prefect query failed: Prefect answered 500 on {FLOW_RUNS_ROUTE}: boom"),
        String::new(),
        "Most recent decision".to_string(),
        format!("Loki query failed: Loki answered 500 on {LOKI_PATH}: boom"),
    ];
    lines.extend(HINT.map(String::from));
    lines.push(String::new());
    assert_eq!(stdout(&output), lines.join("\n"));
    assert_eq!(stub.requests().len(), 4);
}

#[test]
fn cycle_says_in_each_panel_that_there_is_nothing_to_show() {
    let stub = Responses::nothing().serve();
    let copy = Copy::new("cycle-nothing", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let mut lines = vec![
        String::new(),
        "Alert".to_string(),
        "No BgpSessionNotUp alert active for srl1 → 10.1.99.2.".to_string(),
        String::new(),
        "Silences (this peer)".to_string(),
        "No silences scoped to srl1 → 10.1.99.2.".to_string(),
        String::new(),
        "Prefect flow runs (last 30m)".to_string(),
        "No quarantine_bgp / resolved_bgp runs for srl1:10.1.99.2 in the last 30m.".to_string(),
        String::new(),
        "Most recent decision".to_string(),
        "No decision audit records for srl1 → 10.1.99.2 in the last 30m.".to_string(),
    ];
    lines.extend(HINT.map(String::from));
    lines.push(String::new());
    assert_eq!(stdout(&output), lines.join("\n"));
    assert_eq!(stub.requests().len(), 4);
}

#[test]
fn cycle_with_trigger_posts_the_payload_waits_for_a_newer_run_and_prints_no_hint() {
    let t = now();
    let newer = run_json("n1", QUARANTINE, &rfc3339(t - 3), None);
    let mut responses = Responses::packt(t);
    responses.flow_runs = vec![
        (200, packt_runs(t, &[])),
        (200, packt_runs(t, &[])),
        (200, packt_runs(t, &[newer])),
    ];
    let stub = responses.serve();
    let copy = Copy::new("cycle-trigger", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2", "--trigger"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let out = stdout(&output);
    let started = clock_of(t - 3);
    assert!(
        out.starts_with(&format!(
            "{POSTED}\nnew flow run started at {started}\n\nAlert\n"
        )),
        "{out}"
    );
    assert!(!out.contains("To drive a fresh cycle"), "{out}");
    let at = |offset: i64| clock_of(t + offset);
    assert_eq!(
        panel(&output, "Prefect flow runs (last 30m)"),
        [
            "Prefect flow runs (last 30m)".to_string(),
            FLOW_RUNS_HEADER.to_string(),
            format!("{started}  COMPLETED  {QUARANTINE}"),
            format!("{}  COMPLETED  {QUARANTINE}", at(-260)),
            format!("{}  COMPLETED    ├─ evidence", at(-259)),
            format!("{}  COMPLETED    ├─ policy", at(-256)),
            format!("{}  COMPLETED    └─ action", at(-255)),
        ]
        .join("\n")
    );
    let requests = stub.requests();
    assert_eq!(
        order(&requests),
        [
            format!("POST {FLOW_RUNS_ROUTE}"),
            format!("POST {WEBHOOK_ROUTE}"),
            format!("POST {FLOW_RUNS_ROUTE}"),
            format!("POST {FLOW_RUNS_ROUTE}"),
            format!("GET {ALERTS_ROUTE}"),
            format!("GET {SILENCES_ROUTE}"),
            format!("POST {FLOW_RUNS_ROUTE}"),
            format!("POST {TASK_RUNS_ROUTE}"),
            format!("GET {LOKI_PATH}"),
        ]
    );
    assert_eq!(requests[1].header("content-type"), Some("application/json"));
    let body = String::from_utf8_lossy(&requests[1].body).into_owned();
    assert!(
        body.contains(r#""groupKey":"cycle-srl1-10.1.99.2""#),
        "{body}"
    );
    assert!(body.contains(r#""status":"firing""#), "{body}");
}

#[test]
fn cycle_with_trigger_stops_after_the_post_when_the_webhook_answers_500() {
    let t = now();
    let mut responses = Responses::packt(t);
    responses.webhook = (500, "boom".to_string());
    let stub = responses.serve();
    let copy = Copy::new("cycle-webhook-500", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2", "--trigger"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), BOOM_WEBHOOK);
    assert_eq!(
        order(&stub.requests()),
        [
            format!("POST {FLOW_RUNS_ROUTE}"),
            format!("POST {WEBHOOK_ROUTE}")
        ]
    );
}

#[test]
fn cycle_with_trigger_takes_the_current_time_as_the_baseline_when_the_first_query_fails() {
    let t = now();
    let newer = run_json("n1", QUARANTINE, &rfc3339(t + 30), None);
    let mut responses = Responses::packt(t);
    responses.flow_runs = vec![
        (500, "boom".to_string()),
        (200, packt_runs(t, &[])),
        (200, packt_runs(t, &[newer])),
    ];
    let stub = responses.serve();
    let copy = Copy::new("cycle-baseline-500", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2", "--trigger"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.starts_with(&format!(
            "{POSTED}\nnew flow run started at {}\n\nAlert\n",
            clock_of(t + 30)
        )),
        "{out}"
    );
}

#[test]
fn cycle_refuses_a_status_other_than_firing_or_resolved_with_or_without_trigger() {
    let stub = Responses::packt(now()).serve();
    let copy = Copy::new("cycle-status", &cycle_env(stub.url()));

    for args in [
        ["cycle", "srl1", "10.1.99.2", "--status", "later"].as_slice(),
        [
            "cycle",
            "srl1",
            "10.1.99.2",
            "--status",
            "later",
            "--trigger",
        ]
        .as_slice(),
    ] {
        let output = run(&copy, args);

        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(stdout(&output), "");
        assert!(
            stderr(&output).contains("invalid value 'later'"),
            "{}",
            stderr(&output)
        );
    }
    assert!(stub.requests().is_empty());
}

#[test]
fn cycle_takes_every_address_from_its_flag_and_posts_the_status_it_was_given() {
    let t = now();
    let newer = run_json("n1", QUARANTINE, &rfc3339(t - 3), None);
    let mut responses = Responses::packt(t);
    responses.flow_runs = vec![(200, packt_runs(t, &[])), (200, packt_runs(t, &[newer]))];
    let stub = responses.serve();
    let copy = Copy::new("cycle-flags", &cycle_env(&closed_port_url()));
    let webhook = format!("{}{WEBHOOK_ROUTE}", stub.url());

    let output = run(
        &copy,
        &[
            "cycle",
            "srl1",
            "10.1.99.2",
            "--trigger",
            "--status",
            "resolved",
            "--am-url",
            stub.url(),
            "--loki-url",
            stub.url(),
            "--prefect-url",
            stub.url(),
            "--webhook-url",
            &webhook,
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let requests = stub.requests();
    assert_eq!(requests.len(), 8);
    let body = String::from_utf8_lossy(&requests[1].body).into_owned();
    assert!(body.contains(r#""status":"resolved""#), "{body}");
    assert!(!body.contains("firing"), "{body}");
}

#[test]
fn cycle_shows_at_most_five_runs_each_on_its_own_row_and_asks_for_no_task_runs_without_a_parent() {
    let t = now();
    let mut responses = Responses::packt(t);
    let offsets = [100, 120, 140, 160, 180, 260];
    let runs: Vec<String> = offsets
        .iter()
        .enumerate()
        .map(|(index, offset)| {
            run_json(
                &format!("q{index}"),
                &format!("flow_{index} | srl1:10.1.99.2"),
                &rfc3339(t - offset),
                None,
            )
        })
        .collect();
    responses.flow_runs = vec![(200, format!("[{}]", runs.join(",")))];
    let stub = responses.serve();
    let copy = Copy::new("cycle-flat", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let mut rows = vec![
        "Prefect flow runs (last 30m)".to_string(),
        "Started   State      Flow".to_string(),
    ];
    rows.extend(offsets.iter().enumerate().take(5).map(|(index, offset)| {
        format!(
            "{}  COMPLETED  flow_{index} | srl1:10.1.99.2",
            clock_of(t - offset)
        )
    }));
    assert_eq!(
        panel(&output, "Prefect flow runs (last 30m)"),
        rows.join("\n")
    );
    assert!(!order(&stub.requests()).contains(&format!("POST {TASK_RUNS_ROUTE}")));
}

#[test]
fn cycle_shows_a_flat_list_when_the_task_runs_cannot_be_read() {
    let t = now();
    let mut responses = Responses::packt(t);
    responses.task_runs = (500, "boom".to_string());
    let stub = responses.serve();
    let copy = Copy::new("cycle-task-runs-500", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let at = |offset: i64| clock_of(t + offset);
    assert_eq!(
        panel(&output, "Prefect flow runs (last 30m)"),
        [
            "Prefect flow runs (last 30m)".to_string(),
            FLOW_RUNS_HEADER.to_string(),
            format!("{}  COMPLETED  action | srl1:10.1.99.2", at(-255)),
            format!("{}  COMPLETED  policy | srl1:10.1.99.2", at(-256)),
            format!("{}  COMPLETED  evidence | srl1:10.1.99.2", at(-259)),
            format!("{}  COMPLETED  {QUARANTINE}", at(-260)),
        ]
        .join("\n")
    );
    assert!(order(&stub.requests()).contains(&format!("POST {TASK_RUNS_ROUTE}")));
}

#[test]
fn cycle_lists_the_silences_of_this_peer_newest_first_at_most_five_with_every_state() {
    let t = now();
    let silence = |digit: &str, state: &str, age: i64, peer: &str| {
        let id = if digit == "0" {
            String::new()
        } else {
            format!("{}-0000-0000-0000-000000000000", digit.repeat(8))
        };
        silence_json(&id, state, t - age, t - age + 60, peer)
    };
    let silences = [
        silence("3", "active", 500, "10.1.99.2"),
        silence("0", "", 200, "10.1.99.2"),
        silence("1", "active", 700, "10.1.99.2"),
        silence("9", "active", 100, "10.1.99.3"),
        silence("5", "expired", 300, "10.1.99.2"),
        silence("2", "active", 600, "10.1.99.2"),
        silence("4", "pending", 400, "10.1.99.2"),
    ];
    let mut responses = Responses::nothing();
    responses.silences = (200, format!("[{}]", silences.join(",")));
    let stub = responses.serve();
    let copy = Copy::new("cycle-silences", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = panel(&output, "Silences (this peer)");
    let lines: Vec<&str> = text.lines().collect();
    let row = |id: &str, state: &str, age: i64| -> Vec<String> {
        [id, state, &clock_of(t - age), &clock_of(t - age + 60), "—"]
            .map(String::from)
            .to_vec()
    };
    assert_eq!(lines.len(), 7, "{text}");
    assert_eq!(
        cells(lines[1]),
        ["ID", "State", "Starts", "Ends", "Remaining"]
    );
    assert_eq!(cells(lines[2]), row("?", "?", 200));
    assert_eq!(cells(lines[3]), row("55555555-000…", "expired", 300));
    assert_eq!(cells(lines[4]), row("44444444-000…", "pending", 400));
    assert_eq!(cells(lines[5]), row("33333333-000…", "active", 500));
    assert_eq!(cells(lines[6]), row("22222222-000…", "active", 600));
}

#[test]
fn cycle_shows_only_the_alerts_of_this_peer_with_their_state_age_and_silences() {
    let t = now();
    let alert = |labels: &[(&str, &str)], state: &str, silenced: &[&str], started: &str| {
        alert_json(labels, &[], state, silenced, started)
    };
    let alerts = [
        alert(
            &bgp(&[("severity", "critical")]),
            "active",
            &[],
            &rfc3339(t - 90),
        ),
        alert(
            &bgp(&[]),
            "unprocessed",
            &["aaaaaaaa-bbbb-cccc", "second", "third"],
            "soon",
        ),
        alert(&bgp(&[]), "", &[], ""),
        alert(&bgp(&[]), "weird", &[], "soon"),
        alert(
            &[
                ("alertname", "BgpSessionNotUp"),
                ("device", "srl1"),
                ("peer_address", "10.1.99.9"),
            ],
            "active",
            &[],
            &rfc3339(t - 90),
        ),
        alert(
            &[
                ("alertname", "InterfaceDown"),
                ("device", "srl1"),
                ("peer_address", "10.1.99.2"),
            ],
            "active",
            &[],
            &rfc3339(t - 90),
        ),
        alert(
            &[
                ("alertname", "BgpSessionNotUp"),
                ("device", "srl2"),
                ("peer_address", "10.1.99.2"),
            ],
            "active",
            &[],
            &rfc3339(t - 90),
        ),
    ];
    let mut responses = Responses::nothing();
    responses.alerts = (200, format!("[{}]", alerts.join(",")));
    let stub = responses.serve();
    let copy = Copy::new("cycle-alerts", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let expected = ["1m30s", "1m31s"].map(|age| {
        [
            "Alert",
            &format!("BgpSessionNotUp · critical · state=firing · age {age}"),
            "silencedBy: (none)",
            "BgpSessionNotUp · ? · state=pending · age ?",
            "silencedBy: aaaaaaaa-bbb… (+2 more)",
            "BgpSessionNotUp · ? · state=? · age ?",
            "silencedBy: (none)",
            "BgpSessionNotUp · ? · state=weird · age ?",
            "silencedBy: (none)",
        ]
        .join("\n")
    });
    let shown = panel(&output, "Alert");
    assert!(expected.contains(&shown), "{shown}");
}

#[test]
fn cycle_says_so_when_the_latest_decision_line_is_not_json() {
    let mut responses = Responses::packt(now());
    responses.loki = (200, loki_reply(&[(NEWER, "not json at all")]));
    let stub = responses.serve();
    let copy = Copy::new("cycle-decision-text", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        panel(&output, "Most recent decision"),
        "Most recent decision\nCould not parse latest decision JSON."
    );
}

#[test]
fn cycle_minutes_sets_the_window_of_the_flow_runs_and_of_the_decision_and_their_titles() {
    let stub = Responses::packt(now()).serve();
    let copy = Copy::new("cycle-minutes", &cycle_env(stub.url()));

    let output = run(&copy, &["cycle", "srl1", "10.1.99.2", "--minutes", "4"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        panel(&output, "Prefect flow runs (last 4m)"),
        "Prefect flow runs (last 4m)\nNo quarantine_bgp / resolved_bgp runs for srl1:10.1.99.2 in the last 4m."
    );
    let requests = stub.requests();
    assert_eq!(window_minutes(&parameters(&requests[3])), 4);
    assert_eq!(requests.len(), 4);
}

#[test]
fn blank_prefect_and_webhook_variables_in_the_shell_give_way_to_the_env_file() {
    let mut responses = Responses::packt(now());
    responses.webhook = (500, "boom".to_string());
    let stub = responses.serve();
    let copy = Copy::new("cycle-blank-variables", &cycle_env(stub.url()));

    let output = run_with(
        &copy,
        &["cycle", "srl1", "10.1.99.2", "--trigger"],
        &[("PREFECT_API_URL", ""), ("WEBHOOK_URL", "  ")],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), BOOM_WEBHOOK);
    assert_eq!(
        order(&stub.requests()),
        [
            format!("POST {FLOW_RUNS_ROUTE}"),
            format!("POST {WEBHOOK_ROUTE}")
        ]
    );
}

fn page_stub(alerts: &[String]) -> Stub {
    Stub::start(&[("GET", ALERTS_ROUTE, 200, &format!("[{}]", alerts.join(",")))])
}

fn page_env(url: &str) -> String {
    format!("ALERTMANAGER_URL={url}\n")
}

fn active_bgp(t: i64, annotations: &[(&str, &str)]) -> String {
    alert_json(
        &bgp(&[("severity", "warning")]),
        annotations,
        "active",
        &[],
        &rfc3339(t - 269),
    )
}

fn lines_of(output: &Output) -> Vec<String> {
    stdout(output).lines().map(str::to_string).collect()
}

#[test]
fn page_prints_the_on_call_page_and_names_the_alerts_command_and_the_guides_folder() {
    let stub = page_stub(&[active_bgp(now(), &[])]);
    let copy = Copy::new("page-bgp", &page_env(stub.url()));

    let output = run(
        &copy,
        &["page", "-d", "srl1", "-p", "10.1.99.2", "--now", "02:14"],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        [
            "on-call",
            "PAGED 02:14",
            "BgpSessionNotUp on srl1 — peer 10.1.99.2",
            "BGP peer 10.1.99.2 not reaching Established",
            "firing for: ~4 min",
            "severity: warning",
            "",
            "You're awake. The dashboard is your only friend.",
            "",
            "  Confirm the alert: nobs nobs-t-one alerts",
            &format!("  Open the runbook:  {}/guides/", copy.shown()),
            "",
        ]
        .join("\n")
    );
    let requests = stub.requests();
    assert_eq!(order(&requests), [format!("GET {ALERTS_ROUTE}")]);
}

#[test]
fn page_prints_the_summary_of_the_annotations_when_the_alert_has_one() {
    let summary = "Peer 10.1.99.2 is down for the demo";
    let stub = page_stub(&[active_bgp(now(), &[("summary", summary)])]);
    let copy = Copy::new("page-summary", &page_env(stub.url()));

    let output = run(&copy, &["page", "--now", "02:14"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(lines_of(&output)[3], summary);
}

#[test]
fn page_exits_1_with_the_no_match_line_when_every_alert_is_suppressed() {
    let suppressed = alert_json(
        &bgp(&[("severity", "warning")]),
        &[],
        "suppressed",
        &[SILENCED_BY],
        &rfc3339(now() - 269),
    );
    let stub = page_stub(&[suppressed]);
    let copy = Copy::new("page-suppressed", &page_env(stub.url()));

    let output = run(&copy, &["page"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), NO_MATCH);
}

#[test]
fn page_takes_the_address_from_its_flag_and_exits_1_when_alertmanager_answers_500() {
    let stub = Stub::start(&[("GET", ALERTS_ROUTE, 500, "boom")]);
    let copy = Copy::new("page-500", &page_env(&closed_port_url()));

    let output = run(&copy, &["page", "--am-url", stub.url()]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: Alertmanager fetch failed: Alertmanager at {} answered 500\n",
            stub.url()
        )
    );
}

#[test]
fn page_filters_by_alertname_device_and_peer_and_an_empty_alertname_filters_nothing() {
    let t = now();
    let flapping = alert_json(
        &[
            ("alertname", "PeerInterfaceFlapping"),
            ("device", "srl2"),
            ("interface", "ethernet-1/1"),
        ],
        &[],
        "active",
        &[],
        &rfc3339(t - 269),
    );
    let stub = page_stub(&[flapping, active_bgp(t, &[])]);
    let copy = Copy::new("page-filters", &page_env(stub.url()));
    let bgp_title = "BgpSessionNotUp on srl1 — peer 10.1.99.2";
    let flapping_title = "PeerInterfaceFlapping on srl2";

    for (args, title) in [
        (vec!["page"], bgp_title),
        (vec!["page", "-a", ""], flapping_title),
        (vec!["page", "-a", "", "-d", "srl1"], bgp_title),
        (vec!["page", "-a", "", "-p", "10.1.99.2"], bgp_title),
        (vec!["page", "-a", "PeerInterfaceFlapping"], flapping_title),
        (
            vec![
                "page",
                "--alert",
                "BgpSessionNotUp",
                "--device",
                "srl1",
                "--peer",
                "10.1.99.2",
            ],
            bgp_title,
        ),
    ] {
        let mut args = args;
        args.extend(["--now", "02:14"]);

        let output = run(&copy, &args);

        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stderr(&output)
        );
        assert_eq!(lines_of(&output)[2], title, "{args:?}");
    }
    let output = run(
        &copy,
        &["page", "-a", "PeerInterfaceFlapping", "--now", "02:14"],
    );
    let lines = lines_of(&output);
    assert_eq!(lines[3], "interface ethernet-1/1 flapping above threshold");
    assert_eq!(lines[5], "severity: unknown");
}

#[test]
fn page_prints_the_utc_clock_when_now_is_not_given() {
    let stub = page_stub(&[active_bgp(now(), &[])]);
    let copy = Copy::new("page-clock", &page_env(stub.url()));
    let before = now();

    let output = run(&copy, &["page"]);

    let after = now();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let printed = lines_of(&output)[1].clone();
    let allowed = [before, after].map(|seconds| format!("PAGED {}", hours_and_minutes(seconds)));
    assert!(allowed.contains(&printed), "{printed} not in {allowed:?}");
}
