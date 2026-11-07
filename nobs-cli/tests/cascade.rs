use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use nobs_core::cascade::{self, Cycle, Peer};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

const PREFIX: &str = "nobs-t-one-cascade-";
const TITLE: &str = "Workshop one";
const LOKI: &str = "http://loki:3001";
const PROMETHEUS: &str = "http://prometheus:9090/api/v1/write";

const INTERFACE_ANSWER: &str = r#"{"id":"a1b2c3d4-0001","name":"nobs-t-one-cascade-srl1-intf-ethernet-1-1","state":"running"}"#;
const BGP_ANSWER: &str =
    r#"{"id":"e5f6a7b8-0002","name":"nobs-t-one-cascade-srl1-bgp-10-1-2-2","state":"running"}"#;
const INCIDENT_ANSWER: &str = r#"{"id":"c0ffee00-0001","name":"incident-link-failover-srl1","state":"running","warnings":["sink is slow"]}"#;
const FINISHED: &str = r#"{"state":"finished"}"#;
const STOPPED: &str = r#"{"state":"stopped"}"#;
const EMPTY_LIST: &str = r#"{"scenarios":[]}"#;

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str, env: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-cascade-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture(), &root.join("nobs-t-one"));
        fs::write(root.join("nobs-t-one").join(".env"), env).unwrap();
        Self(root.canonicalize().unwrap())
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

fn closed_port_url() -> String {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}")
}

fn copy_for(name: &str, stub: &Stub) -> Copy {
    Copy::new(name, &format!("SONDA_SERVER_URL={}\n", stub.url()))
}

fn run(copy: &Copy, args: &[&str]) -> Output {
    run_with(copy, args, &[])
}

fn run_with(copy: &Copy, args: &[&str], variables: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command.arg("nobs-t-one").args(args).current_dir(copy.dir());
    for name in [
        "SONDA_SERVER_URL",
        "SONDA_API_KEY",
        "SONDA_LOKI_SINK_URL",
        "SONDA_PROM_REMOTE_WRITE_URL",
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

fn cascade_stub() -> Stub {
    Stub::start(&[
        ("POST", "/scenarios", 201, INTERFACE_ANSWER),
        ("POST", "/scenarios", 201, BGP_ANSWER),
    ])
}

fn posted(stub: &Stub) -> Vec<String> {
    stub.requests()
        .into_iter()
        .filter(|request| request.method == "POST")
        .map(|request| String::from_utf8(request.body).unwrap())
        .collect()
}

const CYCLE: Cycle = Cycle {
    duration: "4m",
    up: "30s",
    down: "60s",
    delay: "10s",
};

fn interface_body(interface: &str, loki: &str) -> String {
    cascade::interface_cascade(PREFIX, TITLE, "srl1", interface, &CYCLE, loki)
        .unwrap()
        .to_string()
}

fn bgp_body() -> String {
    let peer = Peer {
        address: "10.1.2.2".to_string(),
        asn: "65102".to_string(),
    };
    cascade::bgp_cascade(PREFIX, TITLE, "srl1", &peer, &CYCLE)
        .unwrap()
        .to_string()
}

fn incident_body(duration: &str, prometheus: &str) -> String {
    cascade::link_failover(
        "srl1",
        "ethernet-1/10",
        "ethernet-1/1",
        duration,
        prometheus,
    )
    .to_string()
}

#[test]
fn flap_interface_posts_the_interface_cascade_and_one_bgp_cascade_per_peer_and_says_what_it_did() {
    let stub = cascade_stub();
    let copy = copy_for("flap-default", &stub);

    let output = run(&copy, &["flap-interface"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        posted(&stub),
        [interface_body("ethernet-1/1", LOKI), bgp_body()]
    );
    let url = stub.url();
    assert_eq!(
        stdout(&output),
        format!(
            "Posting inversion cascade for srl1:ethernet-1/1 (peers: 10.1.2.2, duration 4m, cycle 30s up / 60s down) to {url}/scenarios\n\
             CASCADE       SCENARIO_NAME                              ENTRIES\n\
             interface     nobs-t-one-cascade-srl1-intf-ethernet-1-1  a1b2c3d4\n\
             bgp 10.1.2.2  nobs-t-one-cascade-srl1-bgp-10-1-2-2       e5f6a7b8\n\
             registered 2 scenario(s) across 2 cascade body(s); duration 4m\n\
             \x20 Baselines auto-resume when cascade reaches `finished` (~4m).\n\
             \x20 Inspect:  curl {url}/scenarios\n\
             \x20 Stop early: curl -X DELETE {url}/scenarios/<id>\n"
        )
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn no_cascade_posts_only_the_interface_body_and_does_not_read_lab_vars() {
    let list = format!(r#"{{"scenarios":[{INTERFACE_ANSWER},{BGP_ANSWER}]}}"#);
    let stub = Stub::start(&[("POST", "/scenarios", 201, &list)]);
    let copy = copy_for("flap-no-cascade", &stub);
    fs::remove_file(copy.dir().join("lab_vars.yml")).unwrap();

    let output = run(&copy, &["flap-interface", "--no-cascade"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(posted(&stub), [interface_body("ethernet-1/1", LOKI)]);
    let text = stdout(&output);
    assert!(text.contains("(peers: no-cascade, duration 4m,"), "{text}");
    assert!(text.contains("  a1b2c3d4, e5f6a7b8\n"), "{text}");
    assert!(
        text.contains("registered 2 scenario(s) across 1 cascade body(s); duration 4m\n"),
        "{text}"
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn a_link_with_no_healthy_peer_flaps_the_interface_alone_and_warns() {
    let stub = cascade_stub();
    let copy = copy_for("flap-no-peer", &stub);

    let output = run(&copy, &["flap-interface", "--interface", "ethernet-1/11"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(posted(&stub), [interface_body("ethernet-1/11", LOKI)]);
    let text = stdout(&output);
    assert!(
        text.contains("(peers: interface only, duration 4m,"),
        "{text}"
    );
    assert_eq!(
        stderr(&output),
        "nobs: no healthy BGP peers mapped to srl1:ethernet-1/11; running interface flap only.\n"
    );
}

#[test]
fn an_unknown_device_stops_before_lab_vars_is_read_and_before_anything_is_sent() {
    let stub = cascade_stub();
    let copy = copy_for("flap-device", &stub);
    fs::remove_file(copy.dir().join("lab_vars.yml")).unwrap();

    let output = run(&copy, &["flap-interface", "--device", "srl9"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: unknown device srl9; expected one of srl1, srl2\n"
    );
    assert_eq!(stdout(&output), "");
    assert!(stub.requests().is_empty());
}

#[test]
fn a_duration_that_is_not_a_duration_stops_before_anything_is_printed_or_sent() {
    let stub = cascade_stub();
    let copy = copy_for("flap-delay", &stub);

    let output = run(&copy, &["flap-interface", "--cascade-delay", "soon"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "nobs: soon is not a duration\n");
    assert_eq!(stdout(&output), "");
    assert!(stub.requests().is_empty());
}

#[test]
fn a_failed_post_names_the_scenario_and_the_answer_and_leaves_the_rest_unsent() {
    let body = r#"{"error":"conflict","detail":"scenario is still active"}"#;
    let stub = Stub::start(&[("POST", "/scenarios", 409, body)]);
    let copy = copy_for("flap-conflict", &stub);

    let output = run(&copy, &["flap-interface"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: POST nobs-t-one-cascade-srl1-intf-ethernet-1-1 failed: sonda answered 409: {body}\n"
        )
    );
    assert_eq!(posted(&stub).len(), 1);
    assert!(!stdout(&output).contains("registered"));
}

#[test]
fn an_answer_that_is_neither_shape_stops_the_flap_command() {
    let stub = Stub::start(&[("POST", "/scenarios", 201, r#"{"ok":true}"#)]);
    let copy = copy_for("flap-shape", &stub);

    let output = run(&copy, &["flap-interface"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: unexpected response shape: {\"ok\":true}\n"
    );
    assert!(!stdout(&output).contains("registered"));
}

#[test]
fn the_loki_address_comes_from_the_flag_then_the_variable() {
    let stub = Stub::start(&[("POST", "/scenarios", 201, INTERFACE_ANSWER)]);
    let env = format!(
        "SONDA_SERVER_URL={}\nSONDA_LOKI_SINK_URL=http://from-env:3001\n",
        stub.url()
    );
    let copy = Copy::new("flap-loki", &env);

    run(&copy, &["flap-interface", "--no-cascade"]);
    run(
        &copy,
        &[
            "flap-interface",
            "--no-cascade",
            "--loki-url",
            "http://from-flag:3001",
        ],
    );

    assert_eq!(
        posted(&stub),
        [
            interface_body("ethernet-1/1", "http://from-env:3001"),
            interface_body("ethernet-1/1", "http://from-flag:3001"),
        ]
    );
}

#[test]
fn follow_polls_after_a_sleep_until_every_scenario_is_done_and_does_not_print_the_summary() {
    let stub = Stub::start(&[
        ("POST", "/scenarios", 201, INTERFACE_ANSWER),
        ("POST", "/scenarios", 201, BGP_ANSWER),
        ("GET", "/scenarios/a1b2c3d4-0001", 200, STOPPED),
        ("GET", "/scenarios/a1b2c3d4-0001", 200, FINISHED),
        ("GET", "/scenarios/e5f6a7b8-0002", 200, FINISHED),
    ]);
    let copy = copy_for("flap-follow", &stub);
    let started = Instant::now();

    let output = run(&copy, &["flap-interface", "--follow"]);

    assert!(started.elapsed() >= Duration::from_secs(5));
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.ends_with(
            "  a1b2c3d4 state=stopped\n  e5f6a7b8 state=finished\nall cascade scenarios finished or stopped; baselines back to running\n"
        ),
        "{text}"
    );
    assert!(!text.contains("registered"), "{text}");
    let polled: Vec<String> = stub
        .requests()
        .into_iter()
        .filter(|request| request.method == "GET")
        .map(|request| request.path)
        .collect();
    assert_eq!(
        polled,
        [
            "/scenarios/a1b2c3d4-0001".to_string(),
            "/scenarios/e5f6a7b8-0002".to_string()
        ]
    );
}

#[test]
fn a_failed_poll_is_a_warning_and_a_reply_without_a_state_is_unknown_and_the_wait_goes_on() {
    let stub = Stub::start(&[
        ("POST", "/scenarios", 201, INTERFACE_ANSWER),
        ("POST", "/scenarios", 201, BGP_ANSWER),
        ("GET", "/scenarios/*", 500, "boom"),
        ("GET", "/scenarios/*", 200, "{}"),
        ("GET", "/scenarios/*", 200, FINISHED),
    ]);
    let copy = copy_for("flap-poll", &stub);

    let output = run(&copy, &["flap-interface", "--follow"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        "nobs: poll error on a1b2c3d4-0001: sonda answered 500: boom\n"
    );
    let text = stdout(&output);
    assert!(
        text.ends_with(
            "  e5f6a7b8 state=unknown\n  a1b2c3d4 state=finished\n  e5f6a7b8 state=finished\nall cascade scenarios finished or stopped; baselines back to running\n"
        ),
        "{text}"
    );
}

#[test]
fn incident_posts_the_link_failover_body_and_says_what_it_did() {
    let stub = Stub::start(&[("POST", "/scenarios", 201, INCIDENT_ANSWER)]);
    let copy = copy_for("incident-default", &stub);

    let output = run(&copy, &["incident"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(posted(&stub), [incident_body("3m", PROMETHEUS)]);
    let url = stub.url();
    assert_eq!(
        stdout(&output),
        format!(
            "Posting link-failover cascade for srl1 (ethernet-1/10 → ethernet-1/1, duration 3m) to {url}/scenarios\n\
             ID             NAME                         STATE\n\
             c0ffee00-0001  incident-link-failover-srl1  running\n\
             registered 1 scenario(s): c0ffee00-0001\n\
             \x20 Inspect:  curl {url}/scenarios\n\
             \x20 Stop:     curl -X DELETE {url}/scenarios/<id>\n"
        )
    );
    assert_eq!(stderr(&output), "  warning: sink is slow\n");
}

#[test]
fn a_bare_number_of_seconds_gets_its_unit_before_it_is_sent() {
    let stub = Stub::start(&[("POST", "/scenarios", 201, INCIDENT_ANSWER)]);
    let copy = copy_for("incident-duration", &stub);

    let output = run(&copy, &["incident", "--duration", "90"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("duration 90s) to"));
    assert_eq!(posted(&stub), [incident_body("90s", PROMETHEUS)]);
}

#[test]
fn a_kind_other_than_link_failover_is_refused_before_anything_is_sent() {
    let stub = Stub::start(&[("POST", "/scenarios", 201, INCIDENT_ANSWER)]);
    let copy = copy_for("incident-kind", &stub);

    let output = run(&copy, &["incident", "--kind", "other"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: unknown kind other; supported: link-failover\n"
    );
    assert_eq!(stdout(&output), "");
    assert!(stub.requests().is_empty());
}

#[test]
fn an_incident_answer_without_an_entry_is_an_unexpected_shape() {
    for body in [r#"{"ok":true}"#, EMPTY_LIST] {
        let stub = Stub::start(&[("POST", "/scenarios", 201, body)]);
        let copy = copy_for("incident-shape", &stub);

        let output = run(&copy, &["incident"]);

        assert_eq!(output.status.code(), Some(1), "{body}");
        assert_eq!(
            stderr(&output),
            format!("nobs: unexpected response shape: {body}\n")
        );
        assert!(!stdout(&output).contains("registered"), "{body}");
    }
}

#[test]
fn a_failed_incident_post_says_so_with_the_answer() {
    let stub = Stub::start(&[("POST", "/scenarios", 409, "busy")]);
    let copy = copy_for("incident-conflict", &stub);

    let output = run(&copy, &["incident"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: POST /scenarios failed: sonda answered 409: busy\n"
    );
}

#[test]
fn incident_follow_waits_for_finished_only_and_does_not_print_the_summary() {
    let stub = Stub::start(&[
        ("POST", "/scenarios", 201, INCIDENT_ANSWER),
        ("GET", "/scenarios/*", 200, STOPPED),
        ("GET", "/scenarios/*", 200, FINISHED),
    ]);
    let copy = copy_for("incident-follow", &stub);
    let started = Instant::now();

    let output = run(&copy, &["incident", "--follow"]);

    assert!(started.elapsed() >= Duration::from_secs(10));
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.ends_with(
            "  c0ffee00 state=stopped\n  c0ffee00 state=finished\nall scenarios completed\n"
        ),
        "{text}"
    );
    assert!(!text.contains("registered"), "{text}");
    let polled = stub
        .requests()
        .iter()
        .filter(|request| request.method == "GET")
        .count();
    assert_eq!(polled, 2);
}

#[test]
fn the_remote_write_address_comes_from_the_flag_then_the_variable_even_after_a_blank_one() {
    let stub = Stub::start(&[("POST", "/scenarios", 201, INCIDENT_ANSWER)]);
    let env = format!(
        "SONDA_SERVER_URL={}\nSONDA_PROM_REMOTE_WRITE_URL=http://from-env:9090/api/v1/write\n",
        stub.url()
    );
    let copy = Copy::new("incident-prom", &env);

    run_with(&copy, &["incident"], &[("SONDA_PROM_REMOTE_WRITE_URL", "")]);
    run(
        &copy,
        &[
            "incident",
            "--prom-url",
            "http://from-flag:9090/api/v1/write",
        ],
    );

    assert_eq!(
        posted(&stub),
        [
            incident_body("3m", "http://from-env:9090/api/v1/write"),
            incident_body("3m", "http://from-flag:9090/api/v1/write"),
        ]
    );
}

#[test]
fn scenarios_prints_the_table_of_the_list_command() {
    let body = r#"{"scenarios":[
        {"id":"a1","name":"cpu","state":"running","elapsed_secs":12.5,"degraded":false},
        {"id":"bb22","name":"memory","state":"finished","elapsed_secs":3,"degraded":true}
    ]}"#;
    let stub = Stub::start(&[("GET", "/scenarios", 200, body)]);
    let copy = copy_for("scenarios-list", &stub);

    let output = run(&copy, &["scenarios"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "ID    NAME    STATE     ELAPSED  DEGRADED\n\
         a1    cpu     running   12.5     false\n\
         bb22  memory  finished  3        true\n"
    );
}

#[test]
fn scenarios_says_so_after_the_header_when_there_are_none() {
    let stub = Stub::start(&[("GET", "/scenarios", 200, EMPTY_LIST)]);
    let copy = copy_for("scenarios-empty", &stub);

    let output = run(&copy, &["scenarios"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "ID  NAME  STATE  ELAPSED  DEGRADED\nNo scenarios registered.\n"
    );
}

#[test]
fn scenarios_on_a_closed_port_says_it_cannot_reach_the_server_and_exits_1() {
    let url = closed_port_url();
    let copy = Copy::new("scenarios-closed", &format!("SONDA_SERVER_URL={url}\n"));

    let output = run(&copy, &["scenarios"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with(&format!("nobs: cannot reach {url}: ")),
        "{}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "");
}

#[test]
fn the_sonda_url_flag_beats_the_variable_of_the_env_file() {
    let from_env = Stub::start(&[("GET", "/scenarios", 200, EMPTY_LIST)]);
    let from_flag = Stub::start(&[("GET", "/scenarios", 200, EMPTY_LIST)]);
    let copy = copy_for("scenarios-flag", &from_env);

    let output = run(&copy, &["scenarios", "--sonda-url", from_flag.url()]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(from_flag.requests().len(), 1);
    assert!(from_env.requests().is_empty());
}

#[test]
fn the_key_is_sent_as_a_bearer_token_on_every_call_and_the_flag_beats_the_variable() {
    let stub = Stub::start(&[
        ("POST", "/scenarios", 201, INCIDENT_ANSWER),
        ("GET", "/scenarios", 200, EMPTY_LIST),
    ]);
    let env = format!("SONDA_SERVER_URL={}\nSONDA_API_KEY=env-key\n", stub.url());
    let copy = Copy::new("key", &env);

    run(&copy, &["incident"]);
    run(&copy, &["scenarios"]);
    run(&copy, &["incident", "--sonda-api-key", "flag-key"]);

    let keys: Vec<Option<String>> = stub
        .requests()
        .iter()
        .map(|request| request.header("authorization").map(str::to_string))
        .collect();
    assert_eq!(
        keys,
        [
            Some("Bearer env-key".to_string()),
            Some("Bearer env-key".to_string()),
            Some("Bearer flag-key".to_string()),
        ]
    );
}
