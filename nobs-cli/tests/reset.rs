use std::ffi::OsString;
use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use nobs_core::infrahub::annotation;

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

const TOKEN: &str = "secret-token";
const BEARER: &str = "Bearer sonda-key";
const SINK: &str = "http://loki:3001";
const REWRITTEN: &str = "INFRAHUB_ADDRESS rewritten to host-reachable http://localhost:8000";
const TOKEN_REQUIRED: &str =
    "nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).\n";

const FIRST: &str = "Resetting lab to known-good state.";
const LAST: &str = "lab reset complete — both broken peers should be firing within ~60s";
const LOADED: &str = "  Infrahub already loaded (2 devices)";
const CLEARED: &str = "  cleared maintenance on: srl1, srl2";
const REAPPLIED: &str = "  re-applied sonda-server baseline scenarios (idempotent)";
const DETECTED: &str =
    "detected 1 wedged baseline(s) after cascade delete; restarting sonda-server";
const LOGS_WEDGED: &str =
    "sonda-logs srl1 streams look wedged (no Loki activity in last 60s); restarting";

const SCHEMA_LOAD: &str = "/api/schema/load?branch=main";
const SCHEMA: &str = "/api/schema?branch=main";
const GRAPHQL: &str = "/graphql/main";
const APPLIED: &str = r#"{"schema_updated":true}"#;
const KINDS_FOUND: &str = r#"{"nodes":[{"namespace":"Workshop","name":"Device"},{"namespace":"Workshop","name":"Interface"},{"namespace":"Workshop","name":"BgpSession"}]}"#;

const TWO_DEVICES: &str = r#"{"data":{"WorkshopDevice":{"edges":[{"node":{"name":{"value":"srl1"}}},{"node":{"name":{"value":"srl2"}}}]}}}"#;
const NO_KIND: &str = r#"{"data":null,"errors":[{"message":"Cannot query field WorkshopDevice"}]}"#;

const SILENCES: &str = r#"[{"id":"sil-1","status":{"state":"active"},"createdBy":"nobs-t-one","comment":"Workshop test silence"},{"id":"sil-2","status":{"state":"expired"},"createdBy":"nobs-t-one","comment":"Workshop old silence"}]"#;

const EMPTY_LIST: &str = r#"{"scenarios":[]}"#;
const DIRTY_LIST: &str = r#"{"scenarios":[{"id":"id-1","name":"cascade_active","state":"running"},{"id":"id-2","name":"updown_logs_down","state":"running"},{"id":"id-3","name":"incident_latency_ms","state":"running"},{"id":"id-4","name":"incident_backup_link_utilization","state":"running"},{"id":"id-5","name":"bgpPeerState","state":"running"}]}"#;
const PAUSED_LIST: &str = r#"{"scenarios":[{"id":"base-1","name":"bgpPeerState","state":"paused"},{"id":"base-2","name":"bgpPeerState","state":"running"}]}"#;
const WEDGED: &str = r#"{"id":"base-1","state":"paused","pending_ref":{"scenario_name":"nobs-t-one-cascade-srl1-intf-ethernet-1-1","entry_id":"e"}}"#;
const DELETED: &str = r#"{"id":"x","status":"stopped","total_events":1}"#;

const ONE_LINE: &str =
    r#"{"data":{"result":[{"stream":{},"values":[["1700000000000000000","a line"]]}]}}"#;
const NO_LINES: &str = r#"{"data":{"result":[]}}"#;

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str, env: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-reset-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture(), &root.join("nobs-t-one"));
        fs::create_dir_all(root.join("bin")).unwrap();
        let copy = Self(root.canonicalize().unwrap());
        fs::write(copy.dir().join(".env"), env).unwrap();
        let script = copy.bin().join("docker");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" >> \"{}\"\ncase \" $* \" in *\" restart \"*) exit \"${{FAKE_DOCKER_RESTART_EXIT:-${{FAKE_DOCKER_EXIT:-0}}}}\" ;; esac\nexit \"${{FAKE_DOCKER_EXIT:-0}}\"\n",
                copy.0.join("docker.log").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        copy
    }

    fn dir(&self) -> PathBuf {
        self.0.join("nobs-t-one")
    }

    fn bin(&self) -> PathBuf {
        self.0.join("bin")
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Stubs {
    infrahub: Stub,
    alertmanager: Stub,
    sonda: Stub,
    loki: Stub,
}

impl Stubs {
    fn clean() -> Self {
        Self {
            infrahub: infrahub(&clean_answers()),
            alertmanager: alertmanager(200, "[]"),
            sonda: sonda([EMPTY_LIST, EMPTY_LIST]),
            loki: loki(ONE_LINE),
        }
    }

    fn env_with(&self, infrahub: &str) -> String {
        format!(
            "INFRAHUB_ADDRESS={infrahub}\nINFRAHUB_API_TOKEN={TOKEN}\nALERTMANAGER_URL={}\nSONDA_SERVER_URL={}\nSONDA_API_KEY=sonda-key\nLOKI_URL={}\n",
            self.alertmanager.url(),
            self.sonda.url(),
            self.loki.url()
        )
    }

    fn env(&self) -> String {
        self.env_with(self.infrahub.url())
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

fn path_with(dir: &Path) -> OsString {
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}

fn run(copy: &Copy, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command
        .args(["nobs-t-one", "reset"])
        .args(args)
        .current_dir(copy.dir());
    for name in [
        "INFRAHUB_ADDRESS",
        "INFRAHUB_API_TOKEN",
        "ALERTMANAGER_URL",
        "SONDA_SERVER_URL",
        "SONDA_API_KEY",
        "SONDA_LOKI_SINK_URL",
        "LOKI_URL",
        "NOBS_DIR",
        "FAKE_DOCKER_EXIT",
        "FAKE_DOCKER_RESTART_EXIT",
    ] {
        command.env_remove(name);
    }
    command.env("PATH", path_with(&copy.bin()));
    command.envs(env.iter().copied()).output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn lines(list: &[&str]) -> String {
    list.iter().map(|line| format!("{line}\n")).collect()
}

fn docker_calls(copy: &Copy) -> Vec<String> {
    fs::read_to_string(copy.0.join("docker.log"))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn compose_call(copy: &Copy, words: &str) -> String {
    format!(
        "compose --project-name nobs-t-one -f {}/docker-compose.yml {words}",
        copy.dir().display()
    )
}

fn requests(stub: &Stub) -> Vec<String> {
    stub.requests()
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect()
}

fn param(path: &str, name: &str) -> i64 {
    path.split(['?', '&'])
        .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
        .unwrap()
        .parse()
        .unwrap()
}

fn filter_answer(kind: &str, id: Option<&str>, maintenance: bool) -> String {
    match id {
        Some(id) => format!(
            r#"{{"data":{{"{kind}":{{"count":1,"edges":[{{"node":{{"id":"{id}","maintenance":{{"value":{maintenance}}}}}}}]}}}}}}"#
        ),
        None => format!(r#"{{"data":{{"{kind}":{{"count":0,"edges":[]}}}}}}"#),
    }
}

fn mutation_answer(operation: &str, id: &str) -> String {
    format!(r#"{{"data":{{"{operation}":{{"ok":true,"object":{{"id":"{id}"}}}}}}}}"#)
}

fn seed_answers() -> Vec<String> {
    let mut answers = Vec::new();
    for device in 0..2 {
        for (kind, rows) in [
            ("WorkshopDevice", 1),
            ("WorkshopInterface", 4),
            ("WorkshopBgpSession", 3),
        ] {
            for row in 0..rows {
                let id = format!("{kind}-{device}-{row}");
                answers.push(filter_answer(kind, None, false));
                answers.push(mutation_answer(&format!("{kind}Create"), &id));
            }
        }
    }
    answers
}

fn maintenance_answers() -> Vec<String> {
    vec![
        filter_answer("WorkshopDevice", Some("node-1"), true),
        mutation_answer("WorkshopDeviceUpdate", "node-1"),
    ]
}

fn clean_answers() -> Vec<String> {
    [
        vec![TWO_DEVICES.to_string()],
        maintenance_answers(),
        maintenance_answers(),
    ]
    .concat()
}

fn dirty_answers() -> Vec<String> {
    [
        vec![NO_KIND.to_string()],
        seed_answers(),
        maintenance_answers(),
        maintenance_answers(),
    ]
    .concat()
}

fn infrahub(graphql: &[String]) -> Stub {
    let mut routes = vec![
        ("POST", SCHEMA_LOAD, 200, APPLIED),
        ("GET", SCHEMA, 200, KINDS_FOUND),
    ];
    routes.extend(
        graphql
            .iter()
            .map(|answer| ("POST", GRAPHQL, 200, answer.as_str())),
    );
    Stub::start(&routes)
}

fn alertmanager(status: u16, silences: &str) -> Stub {
    Stub::start(&[
        ("GET", "/api/v2/silences", status, silences),
        ("DELETE", "/api/v2/silence/*", 200, ""),
    ])
}

fn sonda(lists: [&str; 2]) -> Stub {
    Stub::start(&[
        ("GET", "/scenarios", 200, lists[0]),
        ("GET", "/scenarios", 200, lists[1]),
        ("GET", "/scenarios/*", 200, WEDGED),
        ("DELETE", "/scenarios/*", 200, DELETED),
        ("POST", "/events", 200, "{}"),
    ])
}

fn loki(body: &str) -> Stub {
    Stub::start(&[("GET", "/loki/api/v1/query_range?*", 200, body)])
}

fn wedged_sonda_lab() -> Stubs {
    Stubs {
        sonda: sonda([EMPTY_LIST, PAUSED_LIST]),
        ..Stubs::clean()
    }
}

#[test]
fn a_clean_lab_prints_four_lines_calls_docker_once_and_exits_0() {
    let stubs = Stubs::clean();
    let copy = Copy::new("clean", &stubs.env());

    let output = run(&copy, &[], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        lines(&[FIRST, LOADED, CLEARED, REAPPLIED, LAST])
    );
    assert_eq!(docker_calls(&copy), [compose_call(&copy, "up sonda-setup")]);
    assert_eq!(
        requests(&stubs.sonda),
        [
            "POST /events",
            "POST /events",
            "GET /scenarios",
            "GET /scenarios"
        ]
    );
    assert_eq!(requests(&stubs.alertmanager), ["GET /api/v2/silences"]);

    let events: Vec<String> = stubs
        .sonda
        .requests()
        .iter()
        .filter(|request| request.path == "/events")
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    assert_eq!(
        events,
        [
            annotation("srl1", false, SINK),
            annotation("srl2", false, SINK)
        ]
    );
    for request in stubs.sonda.requests() {
        assert_eq!(request.header("Authorization"), Some(BEARER));
    }
    for request in stubs.infrahub.requests() {
        assert_eq!(request.header("X-INFRAHUB-KEY"), Some(TOKEN));
    }

    let asked = stubs.loki.requests();
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0].path.starts_with(
            "/loki/api/v1/query_range?query=%7Bdevice%3D%22srl1%22%2C%20pipeline%3D%22direct%22%7D&"
        ),
        "{}",
        asked[0].path
    );
    assert_eq!(param(&asked[0].path, "limit"), 1);
    assert_eq!(
        param(&asked[0].path, "end") - param(&asked[0].path, "start"),
        60_000_000_000
    );
}

#[test]
fn a_dirty_lab_is_loaded_cleared_expired_deleted_and_restarted_in_order() {
    let stubs = Stubs {
        infrahub: infrahub(&dirty_answers()),
        alertmanager: alertmanager(200, SILENCES),
        sonda: sonda([DIRTY_LIST, PAUSED_LIST]),
        loki: loki(NO_LINES),
    };
    let copy = Copy::new("dirty", &stubs.env());
    let started = Instant::now();

    let output = run(&copy, &[], &[]);

    assert!(started.elapsed() >= Duration::from_secs(5));
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        lines(&[
            FIRST,
            "Infrahub has 0 WorkshopDevice record(s); re-loading schema + data",
            "  re-loaded Infrahub schema + data (2 device(s) seeded)",
            CLEARED,
            REAPPLIED,
            "  expired 1 workshop-related Alertmanager silence(s)",
            "  deleted 4 lingering cascade scenario(s)",
            DETECTED,
            "  restarted sonda-server and re-applied baselines",
            LOGS_WEDGED,
            "  restarted sonda-logs (loki sink reset)",
            LAST,
        ])
    );
    assert_eq!(
        docker_calls(&copy),
        [
            compose_call(&copy, "up sonda-setup"),
            compose_call(&copy, "restart sonda-server"),
            compose_call(&copy, "up sonda-setup"),
            compose_call(&copy, "restart sonda-logs"),
        ]
    );
    assert_eq!(
        requests(&stubs.sonda),
        [
            "POST /events",
            "POST /events",
            "GET /scenarios",
            "DELETE /scenarios/id-1",
            "DELETE /scenarios/id-2",
            "DELETE /scenarios/id-3",
            "DELETE /scenarios/id-4",
            "GET /scenarios",
            "GET /scenarios/base-1",
        ]
    );
    assert_eq!(
        requests(&stubs.alertmanager),
        ["GET /api/v2/silences", "DELETE /api/v2/silence/sil-1"]
    );
    let mut expected = vec![
        format!("POST {GRAPHQL}"),
        format!("POST {SCHEMA_LOAD}"),
        format!("GET {SCHEMA}"),
    ];
    expected.extend((0..36).map(|_| format!("POST {GRAPHQL}")));
    assert_eq!(requests(&stubs.infrahub), expected);
    assert_eq!(stubs.loki.requests().len(), 1);
}

#[test]
fn skip_sonda_logs_neither_asks_loki_nor_restarts_sonda_logs() {
    let stubs = Stubs {
        loki: loki(NO_LINES),
        ..Stubs::clean()
    };
    let copy = Copy::new("skip-logs", &stubs.env());

    let output = run(&copy, &["--skip-sonda-logs"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        lines(&[FIRST, LOADED, CLEARED, REAPPLIED, LAST])
    );
    assert_eq!(stubs.loki.requests().len(), 0);
    assert_eq!(docker_calls(&copy), [compose_call(&copy, "up sonda-setup")]);
}

#[test]
fn an_infrahub_that_is_down_warns_three_times_and_the_other_steps_still_run() {
    let stubs = Stubs::clean();
    let closed = closed_port_url();
    let copy = Copy::new("infrahub-down", &stubs.env_with(&closed));

    let output = run(&copy, &[], &[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), lines(&[FIRST, REAPPLIED, LAST]));
    let errors = stderr(&output);
    let errors: Vec<&str> = errors.lines().collect();
    assert_eq!(errors.len(), 3, "{errors:?}");
    let probe = format!("nobs: Infrahub probe failed: cannot reach {closed}: ");
    assert!(errors[0].starts_with(&probe), "{}", errors[0]);
    assert!(
        errors[0].ends_with(" — skipping load check"),
        "{}",
        errors[0]
    );
    for (line, device) in errors[1..].iter().zip(["srl1", "srl2"]) {
        let failed =
            format!("nobs: maintenance --clear failed for {device}: cannot reach {closed}: ");
        assert!(line.starts_with(&failed), "{line}");
    }
    assert_eq!(docker_calls(&copy), [compose_call(&copy, "up sonda-setup")]);
    assert_eq!(requests(&stubs.sonda), ["GET /scenarios", "GET /scenarios"]);
    assert_eq!(requests(&stubs.alertmanager), ["GET /api/v2/silences"]);
    assert_eq!(stubs.loki.requests().len(), 1);
}

#[test]
fn without_a_token_nothing_is_sent_nothing_is_run_and_the_last_line_is_missing() {
    let stubs = Stubs::clean();
    let without_token = stubs
        .env()
        .replace(&format!("INFRAHUB_API_TOKEN={TOKEN}\n"), "");
    let copy = Copy::new("no-token", &without_token);

    let output = run(&copy, &[], &[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), lines(&[FIRST]));
    assert_eq!(stderr(&output), TOKEN_REQUIRED);
    for stub in [
        &stubs.infrahub,
        &stubs.alertmanager,
        &stubs.sonda,
        &stubs.loki,
    ] {
        assert_eq!(stub.requests().len(), 0);
    }
    assert_eq!(docker_calls(&copy), Vec::<String>::new());
}

#[test]
fn the_rewrite_line_follows_the_first_line_and_comes_before_the_token_check() {
    let stubs = Stubs::clean();
    let without_token = stubs
        .env_with("http://infrahub-server:8000")
        .replace(&format!("INFRAHUB_API_TOKEN={TOKEN}\n"), "");
    let copy = Copy::new("rewrite", &without_token);

    let output = run(&copy, &[], &[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), lines(&[FIRST, REWRITTEN]));
    assert_eq!(stderr(&output), TOKEN_REQUIRED);
}

#[test]
fn the_flags_win_over_the_variables_of_the_dot_env_file() {
    let stubs = Stubs::clean();
    let closed = closed_port_url();
    let env = format!(
        "INFRAHUB_ADDRESS={closed}\nINFRAHUB_API_TOKEN=wrong\nALERTMANAGER_URL={closed}\nSONDA_SERVER_URL={closed}\nLOKI_URL={closed}\n"
    );
    let copy = Copy::new("flags", &env);

    let output = run(
        &copy,
        &[
            "--alertmanager-url",
            stubs.alertmanager.url(),
            "--sonda-url",
            stubs.sonda.url(),
            "--loki-url",
            stubs.loki.url(),
            "--infrahub-url",
            stubs.infrahub.url(),
            "--token",
            TOKEN,
        ],
        &[],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        lines(&[FIRST, LOADED, CLEARED, REAPPLIED, LAST])
    );
    for request in stubs.infrahub.requests() {
        assert_eq!(request.header("X-INFRAHUB-KEY"), Some(TOKEN));
    }
}

#[test]
fn the_seeded_count_is_the_number_of_devices_the_lab_file_holds() {
    let answers = [
        vec![
            NO_KIND.to_string(),
            filter_answer("WorkshopDevice", None, false),
            mutation_answer("WorkshopDeviceCreate", "dev-1"),
        ],
        maintenance_answers(),
        maintenance_answers(),
    ]
    .concat();
    let stubs = Stubs {
        infrahub: infrahub(&answers),
        ..Stubs::clean()
    };
    let copy = Copy::new("one-device", &stubs.env());
    fs::write(
        copy.dir().join("lab_vars.yml"),
        "nodes:\n  srl1:\n    asn: 65101\n",
    )
    .unwrap();

    let output = run(&copy, &[], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        lines(&[
            FIRST,
            "Infrahub has 0 WorkshopDevice record(s); re-loading schema + data",
            "  re-loaded Infrahub schema + data (1 device(s) seeded)",
            CLEARED,
            REAPPLIED,
            LAST,
        ])
    );
}

#[test]
fn a_failed_load_warns_with_the_load_error_and_prints_no_reloaded_line() {
    let answers = [
        vec![NO_KIND.to_string()],
        maintenance_answers(),
        maintenance_answers(),
    ]
    .concat();
    let stubs = Stubs {
        infrahub: infrahub(&answers),
        ..Stubs::clean()
    };
    let copy = Copy::new("load-fails", &stubs.env());
    fs::remove_file(copy.dir().join("lab_vars.yml")).unwrap();

    let output = run(&copy, &[], &[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: load-infrahub failed: lab_vars file not found: {}/lab_vars.yml\n",
            copy.dir().display()
        )
    );
    assert_eq!(
        stdout(&output),
        lines(&[
            FIRST,
            "Infrahub has 0 WorkshopDevice record(s); re-loading schema + data",
            CLEARED,
            REAPPLIED,
            LAST,
        ])
    );
}

#[test]
fn a_loki_that_fails_warns_and_sonda_logs_is_not_restarted() {
    let stubs = Stubs {
        loki: Stub::start(&[("GET", "/loki/api/v1/query_range?*", 500, "boom")]),
        ..Stubs::clean()
    };
    let copy = Copy::new("loki-500", &stubs.env());

    let output = run(&copy, &[], &[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: Loki probe failed: Loki answered 500 on /loki/api/v1/query_range: boom — skipping sonda-logs check\n"
    );
    assert_eq!(
        stdout(&output),
        lines(&[FIRST, LOADED, CLEARED, REAPPLIED, LAST])
    );
    assert_eq!(docker_calls(&copy), [compose_call(&copy, "up sonda-setup")]);
}

#[test]
fn a_failing_sonda_setup_warns_with_its_exit_code_and_exits_1() {
    let stubs = Stubs::clean();
    let copy = Copy::new("setup-fails", &stubs.env());

    let output = run(&copy, &[], &[("FAKE_DOCKER_EXIT", "1")]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), lines(&[FIRST, LOADED, CLEARED, LAST]));
    assert_eq!(
        stderr(&output),
        "nobs: sonda-setup up failed: exited with code 1\n"
    );
}

#[test]
fn an_alertmanager_answering_500_warns_and_the_sonda_steps_still_run() {
    let stubs = Stubs {
        alertmanager: alertmanager(500, "boom"),
        ..Stubs::clean()
    };
    let copy = Copy::new("alertmanager-500", &stubs.env());

    let output = run(&copy, &[], &[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: Alertmanager fetch failed: Alertmanager at {} answered 500 — skipping silence cleanup\n",
            stubs.alertmanager.url()
        )
    );
    assert_eq!(
        stdout(&output),
        lines(&[FIRST, LOADED, CLEARED, REAPPLIED, LAST])
    );
    assert_eq!(
        requests(&stubs.sonda),
        [
            "POST /events",
            "POST /events",
            "GET /scenarios",
            "GET /scenarios"
        ]
    );
    assert_eq!(stubs.loki.requests().len(), 1);
}

#[test]
fn a_failed_sonda_server_restart_ends_the_wedge_step_without_a_success_line() {
    let stubs = wedged_sonda_lab();
    let copy = Copy::new("restart-fails", &stubs.env());

    let output = run(&copy, &[], &[("FAKE_DOCKER_RESTART_EXIT", "1")]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: sonda-server restart failed: exited with code 1\n"
    );
    assert_eq!(
        stdout(&output),
        lines(&[FIRST, LOADED, CLEARED, REAPPLIED, DETECTED, LAST])
    );
    assert_eq!(
        docker_calls(&copy),
        [
            compose_call(&copy, "up sonda-setup"),
            compose_call(&copy, "restart sonda-server"),
        ]
    );
}

#[test]
fn a_scenario_list_nobs_cannot_read_warns_for_both_fetches_and_deletes_nothing() {
    let stubs = Stubs {
        sonda: sonda(["[]", "[]"]),
        ..Stubs::clean()
    };
    let copy = Copy::new("unreadable-list", &stubs.env());

    let output = run(&copy, &[], &[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: Sonda /scenarios fetch failed: sonda answered a list nobs cannot read: the reply has no \"scenarios\" list — skipping cascade cleanup\nnobs: Sonda /scenarios fetch failed: sonda answered a list nobs cannot read: the reply has no \"scenarios\" list — skipping wedge check\n"
    );
    assert_eq!(
        requests(&stubs.sonda),
        [
            "POST /events",
            "POST /events",
            "GET /scenarios",
            "GET /scenarios"
        ]
    );
}

#[test]
fn a_missing_compose_file_warns_for_every_docker_step_and_the_http_steps_still_run() {
    let stubs = Stubs {
        loki: loki(NO_LINES),
        ..wedged_sonda_lab()
    };
    let copy = Copy::new("no-compose", &stubs.env());
    fs::remove_file(copy.dir().join("docker-compose.yml")).unwrap();

    let output = run(&copy, &[], &[]);

    assert_eq!(output.status.code(), Some(1));
    let missing = format!(
        "nobs: compose file not found: {}/docker-compose.yml\n",
        copy.dir().display()
    );
    assert_eq!(stderr(&output), missing.repeat(3));
    assert_eq!(
        stdout(&output),
        lines(&[FIRST, LOADED, CLEARED, DETECTED, LOGS_WEDGED, LAST])
    );
    assert_eq!(docker_calls(&copy), Vec::<String>::new());
    assert_eq!(
        requests(&stubs.sonda),
        [
            "POST /events",
            "POST /events",
            "GET /scenarios",
            "GET /scenarios",
            "GET /scenarios/base-1"
        ]
    );
    assert_eq!(stubs.loki.requests().len(), 1);
}

#[test]
fn the_help_names_the_command_and_the_six_flags_with_their_texts() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["nobs-t-one", "reset", "--help"])
        .current_dir(fixture())
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let help = stdout(&output);
    assert!(
        help.starts_with("Return the lab to the workshop's known-good baseline\n"),
        "{help}"
    );
    let flags = [
        (
            "--alertmanager-url <URL>",
            "Address of Alertmanager; nobs looks at --alertmanager-url, then ALERTMANAGER_URL, then http://localhost:9093",
        ),
        (
            "--sonda-url <URL>",
            "Address of Sonda; nobs looks at --sonda-url, then SONDA_SERVER_URL, then http://localhost:8085",
        ),
        (
            "--loki-url <URL>",
            "Address of Loki; nobs looks at --loki-url, then LOKI_URL, then http://localhost:3001",
        ),
        (
            "--infrahub-url <URL>",
            "Address of Infrahub; nobs looks at --infrahub-url, then INFRAHUB_ADDRESS, then http://localhost:8000",
        ),
        (
            "--skip-sonda-logs",
            "Don't restart sonda-logs even if the loki sink looks wedged",
        ),
        (
            "--token <TOKEN>",
            "Token for Infrahub; nobs looks at --token, then INFRAHUB_API_TOKEN",
        ),
    ];
    for (flag, text) in flags {
        assert!(
            help.lines()
                .any(|line| line.trim_start().starts_with(flag) && line.ends_with(text)),
            "{flag}\n{help}"
        );
    }
}
