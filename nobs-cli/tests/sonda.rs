use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

const NOT_FOUND: &str = r#"{"error":"not_found","detail":"scenario not found: x"}"#;

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/sonda")
        .join(name)
        .to_str()
        .unwrap()
        .to_string()
}

fn nobs(server: &str, args: &[&str]) -> Output {
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sonda-no-dot-env");
    std::fs::create_dir_all(&folder).unwrap();
    Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["sonda", "--server", server])
        .args(args)
        .env_remove("SONDA_SERVER_URL")
        .env_remove("SONDA_API_KEY")
        .current_dir(folder)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn list_prints_a_header_and_one_line_per_scenario() {
    let body = r#"{"scenarios":[
        {"id":"a1","name":"cpu","state":"running","elapsed_secs":12.5,"degraded":false},
        {"id":"bb22","name":"memory","state":"finished","elapsed_secs":3,"degraded":true}
    ]}"#;
    let stub = Stub::start(&[("GET", "/scenarios", 200, body)]);

    let output = nobs(stub.url(), &["list"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "ID    NAME    STATE     ELAPSED  DEGRADED\n\
         a1    cpu     running   12.5     false\n\
         bb22  memory  finished  3        true\n"
    );
}

#[test]
fn the_server_flag_is_accepted_after_the_command() {
    let stub = Stub::start(&[("GET", "/scenarios", 200, r#"{"scenarios":[]}"#)]);

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["sonda", "list", "--server", stub.url()])
        .env_remove("SONDA_SERVER_URL")
        .env_remove("SONDA_API_KEY")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stub.requests().len(), 1);
}

#[test]
fn list_prints_only_the_header_for_an_empty_list() {
    let stub = Stub::start(&[("GET", "/scenarios", 200, r#"{"scenarios":[]}"#)]);

    let output = nobs(stub.url(), &["list"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "ID  NAME  STATE  ELAPSED  DEGRADED\n");
}

#[test]
fn list_exits_1_on_a_body_that_is_not_a_list() {
    let stub = Stub::start(&[("GET", "/scenarios", 200, r#"{"items":[]}"#)]);

    let output = nobs(stub.url(), &["list"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("cannot read"),
        "{}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "");
}

#[test]
fn show_prints_the_body_one_field_per_line() {
    let stub = Stub::start(&[(
        "GET",
        "/scenarios/a1",
        200,
        r#"{"state":"running","id":"a1"}"#,
    )]);

    let output = nobs(stub.url(), &["show", "a1"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "{\n  \"id\": \"a1\",\n  \"state\": \"running\"\n}\n"
    );
}

#[test]
fn ready_asks_get_ready_and_prints_the_reply() {
    let stub = Stub::start(&[("GET", "/ready", 200, r#"{"status":"not_configured"}"#)]);

    let output = nobs(stub.url(), &["ready"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "{\n  \"status\": \"not_configured\"\n}\n");
    let seen = stub.requests();
    assert_eq!(
        (seen[0].method.as_str(), seen[0].path.as_str()),
        ("GET", "/ready")
    );
}

#[test]
fn stats_asks_the_stats_path_of_the_scenario_and_prints_the_reply() {
    let stub = Stub::start(&[("GET", "/scenarios/a1/stats", 200, r#"{"b":2,"a":1}"#)]);

    let output = nobs(stub.url(), &["stats", "a1"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "{\n  \"a\": 1,\n  \"b\": 2\n}\n");
    let seen = stub.requests();
    assert_eq!(
        (seen[0].method.as_str(), seen[0].path.as_str()),
        ("GET", "/scenarios/a1/stats")
    );
}

#[test]
fn metrics_prints_the_text_unchanged() {
    let text = "# TYPE up gauge\nup 1\n";
    let stub = Stub::start(&[("GET", "/metrics", 200, text)]);

    let output = nobs(stub.url(), &["metrics"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), text);
    let seen = stub.requests();
    assert_eq!(
        (seen[0].method.as_str(), seen[0].path.as_str()),
        ("GET", "/metrics")
    );
}

#[test]
fn load_posts_the_file_unchanged_and_prints_the_reply() {
    let stub = Stub::start(&[("POST", "/scenarios", 201, r#"{"id":"x1"}"#)]);
    let file = fixture("hello.yaml");

    let output = nobs(stub.url(), &["load", &file]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "{\n  \"id\": \"x1\"\n}\n");
    let seen = stub.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].body, std::fs::read(&file).unwrap());
}

#[test]
fn load_refuses_a_file_that_is_not_runnable_and_sends_nothing() {
    let cases = [
        ("pack.yaml", "pack"),
        ("no-kind.yaml", "no kind: line"),
        ("hello.json", "JSON"),
    ];

    for (name, reason) in cases {
        let stub = Stub::start(&[("POST", "/scenarios", 201, r#"{"id":"x1"}"#)]);

        let output = nobs(stub.url(), &["load", &fixture(name)]);

        assert_eq!(output.status.code(), Some(1), "{name}");
        let message = stderr(&output);
        assert!(message.contains(name), "{name}: {message}");
        assert!(message.contains(reason), "{name}: {message}");
        assert!(stub.requests().is_empty(), "{name}");
    }
}

#[test]
fn load_of_a_missing_file_exits_1_names_the_path_and_sends_nothing() {
    let stub = Stub::start(&[("POST", "/scenarios", 201, r#"{"id":"x1"}"#)]);

    let output = nobs(stub.url(), &["load", "nobs-t-missing.yaml"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("nobs-t-missing.yaml"),
        "{}",
        stderr(&output)
    );
    assert!(stub.requests().is_empty());
}

#[test]
fn event_posts_the_file_unchanged_as_json() {
    let stub = Stub::start(&[("POST", "/events", 200, r#"{"sent":true}"#)]);
    let file = fixture("event-stdout.json");

    let output = nobs(stub.url(), &["event", &file]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let seen = stub.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].body, std::fs::read(&file).unwrap());
    assert_eq!(seen[0].header("Content-Type"), Some("application/json"));
}

#[test]
fn delete_of_an_unknown_scenario_exits_1_and_prints_the_server_text() {
    let stub = Stub::start(&[("DELETE", "/scenarios/x", 404, NOT_FOUND)]);

    let output = nobs(stub.url(), &["delete", "x"]);

    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(message.contains("sonda answered 404"), "{message}");
    assert!(message.contains("not_found"), "{message}");
    assert_eq!(stdout(&output), "");
}

#[test]
fn version_prints_the_server_version_and_sha() {
    let metrics = "# TYPE sonda_server_build_info gauge\nsonda_server_build_info{version=\"1.29.0\",git_sha=\"abc\"} 1\n";
    let stub = Stub::start(&[("GET", "/metrics", 200, metrics)]);

    let output = nobs(stub.url(), &["version"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "sonda-server 1.29.0 abc\n");
}

#[test]
fn version_exits_1_when_the_metrics_have_no_build_info() {
    let stub = Stub::start(&[("GET", "/metrics", 200, "up 1\n")]);

    let output = nobs(stub.url(), &["version"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("without sonda_server_build_info"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn health_against_a_closed_port_exits_1_and_says_cannot_reach() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    let output = nobs(&format!("http://127.0.0.1:{port}"), &["health"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("cannot reach"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_hello_fixture_is_a_runnable_scenario_named_for_the_tests() {
    let text = std::fs::read_to_string(fixture("hello.yaml")).unwrap();

    for part in [
        "kind: runnable",
        "scenario_name: nobs-t-hello",
        "duration: 10m",
        "type: stdout",
    ] {
        assert!(text.contains(part), "{part}");
    }
}

#[test]
fn the_other_fixtures_have_the_shape_their_names_say() {
    let text = |name: &str| std::fs::read_to_string(fixture(name)).unwrap();

    assert!(text("pack.yaml").contains("kind: composable"));
    assert!(!text("no-kind.yaml")
        .lines()
        .any(|line| line.starts_with("kind:")));
    assert!(text("hello.json").starts_with('{'));
    let event = text("event-stdout.json");
    assert!(event.contains("\"signal_type\": \"metrics\""));
    assert!(event.contains("\"type\": \"stdout\""));
}
