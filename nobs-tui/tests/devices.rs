use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use nobs_core::alertmanager::Alert;
use nobs_core::lab_vars;
use nobs_core::loki::Line;
use nobs_core::prefect::FlowRun;
use nobs_core::prometheus::Sample;
use nobs_core::runner::{Output, Runner};
use nobs_core::sot::{Device, Fixed, SourceOfTruth};
use nobs_core::status::Urls;
use nobs_core::time;
use nobs_tui::devices::{
    alert_count, counts, has_bgp_series, health, join, list_rows, merge, oper_states, Health, Kind,
};
use nobs_tui::{Command, Pollers, SondaTarget, Tab, Update};

#[path = "../../tests/support/stub.rs"]
mod stub;
use stub::Stub;

const WORKSHOP_ONE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/fixtures/workshop-one/lab_vars.yml"
);
const BOOK_LAB: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/fixtures/book-lab/containerlab/lab_vars.yml"
);
const NOW_SECONDS: u64 = 1_794_232_500;

fn fixed(path: &str) -> Fixed {
    let text = fs::read_to_string(path).unwrap();
    Fixed::from_lab_vars(&lab_vars::read(&text).unwrap())
}

fn srl1() -> Device {
    fixed(WORKSHOP_ONE)
        .device("srl1")
        .unwrap()
        .expect("srl1 is in the workshop fixture")
}

fn values(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs
        .iter()
        .map(|(peer, value)| (peer.to_string(), *value))
        .collect()
}

fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn sample(pairs: &[(&str, &str)], value: f64) -> Sample {
    Sample {
        labels: labels(pairs),
        value,
    }
}

fn interface_sample(metric: &str, name: &str, value: f64) -> Sample {
    sample(
        &[("__name__", metric), ("device", "srl1"), ("name", name)],
        value,
    )
}

fn peer_sample(peer: &str, value: f64) -> Sample {
    sample(
        &[
            ("__name__", "bgp_oper_state"),
            ("device", "srl1"),
            ("peer_address", peer),
            ("afi_safi_name", "ipv4-unicast"),
            ("name", "default"),
        ],
        value,
    )
}

fn alert(device: &str, severity: &str, state: &str, silenced: bool, starts_at: &str) -> Alert {
    Alert {
        labels: labels(&[
            ("alertname", "BgpSessionNotUp"),
            ("device", device),
            ("severity", severity),
        ]),
        state: state.to_string(),
        silenced,
        inhibited: false,
        starts_at: starts_at.to_string(),
        silenced_by: vec![],
        annotations: BTreeMap::new(),
    }
}

fn run(name: &str, device: &str, start_time: &str) -> FlowRun {
    FlowRun {
        id: format!("id-{name}"),
        name: name.to_string(),
        device: Some(device.to_string()),
        peer: Some("10.1.99.2".to_string()),
        start_time: start_time.to_string(),
        state: "COMPLETED".to_string(),
        parent_task_run_id: None,
    }
}

fn nanoseconds(time_text: &str) -> i64 {
    time::parse_rfc3339(time_text).unwrap() * 1_000_000_000
}

#[test]
fn the_list_rows_follow_the_source_order_with_the_marker_and_the_dash_kind() {
    let source = fixed(BOOK_LAB);
    source.set_maintenance("nobs-t-srl1", true).unwrap();

    let rows = list_rows(&source.devices().unwrap(), None, None);

    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["nobs-t-srl1", "nobs-t-srl2"]);
    assert!(rows.iter().all(|row| row.kind == "—"), "{rows:?}");
    let marks: Vec<bool> = rows.iter().map(|row| row.maintenance).collect();
    assert_eq!(marks, [true, false]);
    assert!(rows.iter().all(|row| row.health == Health::Unknown));
    assert!(rows.iter().all(|row| row.alerts.is_none()));
}

#[test]
fn the_health_rule_gives_ok_warn_down_and_unknown_from_values() {
    let sessions = srl1().sessions;
    assert_eq!(sessions.len(), 3);
    let all_up = values(&[("10.1.2.2", 1.0), ("10.1.7.2", 1.0), ("10.1.99.2", 1.0)]);
    let one_active = values(&[("10.1.2.2", 1.0), ("10.1.7.2", 1.0), ("10.1.99.2", 5.0)]);
    let one_missing = values(&[("10.1.2.2", 1.0), ("10.1.7.2", 1.0)]);

    let ok = health(&sessions, Some(&all_up), true);
    let not_up = health(&sessions, Some(&one_active), true);
    let missing = health(&sessions, Some(&one_missing), true);
    let down = health(&sessions, Some(&BTreeMap::new()), false);
    let unknown = health(&sessions, None, false);
    let none = health(&[], Some(&all_up), true);

    let health_from = |samples: &[Sample]| {
        health(
            &sessions,
            Some(&oper_states("srl1", &sessions, samples)),
            has_bgp_series("srl1", samples),
        )
    };
    let other_family = sample(
        &[
            ("__name__", "bgp_oper_state"),
            ("device", "srl1"),
            ("peer_address", "10.1.2.2"),
            ("afi_safi_name", "ipv6-unicast"),
            ("name", "default"),
        ],
        1.0,
    );
    let unintended_peer = health_from(&[peer_sample("10.9.9.9", 1.0)]);
    let unintended_family = health_from(&[other_family]);
    let no_series = health_from(&[interface_sample("interface_oper_state", "lo0", 1.0)]);

    assert_eq!(ok, Health::Ok);
    assert_eq!(not_up, Health::Warn);
    assert_eq!(missing, Health::Warn);
    assert_eq!(down, Health::Down);
    assert_eq!(unknown, Health::Unknown);
    assert_eq!(none, Health::NoSessions);
    assert_eq!(unintended_peer, Health::Warn);
    assert_eq!(unintended_family, Health::Warn);
    assert_eq!(no_series, Health::Down);
    let words: Vec<&str> = [ok, not_up, missing, down, unknown, none]
        .into_iter()
        .map(Health::text)
        .collect();
    assert_eq!(words, ["ok", "warn", "warn", "down", "?", "—"]);
}

#[test]
fn the_session_state_comes_from_the_series_of_its_address_family_and_instance() {
    let series = |afi_safi: &str, instance: &str, value: f64| {
        sample(
            &[
                ("__name__", "bgp_oper_state"),
                ("device", "srl1"),
                ("peer_address", "10.1.2.2"),
                ("afi_safi_name", afi_safi),
                ("name", instance),
            ],
            value,
        )
    };
    let samples = [
        series("ipv6-unicast", "default", 5.0),
        series("ipv4-unicast", "other", 2.0),
        series("ipv4-unicast", "default", 1.0),
    ];
    let session = |afi_safi: Option<&str>| {
        let mut session = srl1().sessions.remove(0);
        assert_eq!(session.peer, "10.1.2.2");
        session.afi_safi = afi_safi.map(str::to_string);
        session
    };

    let default_family = oper_states("srl1", &[session(None)], &samples);
    let ipv6 = oper_states("srl1", &[session(Some("ipv6-unicast"))], &samples);
    let other_only = oper_states("srl1", &[session(None)], &samples[1..2]);

    assert_eq!(default_family, values(&[("10.1.2.2", 1.0)]));
    assert_eq!(ipv6, values(&[("10.1.2.2", 5.0)]));
    assert!(other_only.is_empty());
}

#[test]
fn the_alert_count_is_per_device_active_only_and_red_for_a_critical() {
    let at = "2026-11-09T13:53:32Z";
    let mut alerts = vec![
        alert("srl1", "warning", "active", false, at),
        alert("srl1", "warning", "active", false, at),
        alert("srl2", "warning", "active", false, at),
        alert("srl1", "warning", "suppressed", true, at),
        alert("srl1", "warning", "active", true, at),
    ];

    assert_eq!(alert_count(&alerts, "srl1"), (2, false));
    assert_eq!(alert_count(&alerts, "srl2"), (1, false));
    assert_eq!(alert_count(&alerts, "srl9"), (0, false));
    assert_eq!(alerts[3].state_word(), "silenced");
    assert_eq!(alerts[4].state_word(), "silenced");
    assert_eq!(alerts[0].state_word(), "firing");

    alerts[1]
        .labels
        .insert("severity".to_string(), "critical".to_string());

    assert_eq!(alert_count(&alerts, "srl1"), (2, true));
    assert_eq!(alert_count(&alerts, "srl2"), (1, false));
}

#[test]
fn the_join_matches_the_peer_by_subnet_and_decodes_the_live_columns() {
    let device = srl1();
    let samples = vec![
        interface_sample("interface_admin_state", "ethernet-1/1", 1.0),
        interface_sample("interface_oper_state", "ethernet-1/1", 1.0),
        interface_sample("interface_admin_state", "ethernet-1/10", 1.0),
        interface_sample("interface_oper_state", "ethernet-1/10", 1.0),
        interface_sample("interface_admin_state", "ethernet-1/11", 1.0),
        interface_sample("interface_oper_state", "ethernet-1/11", 2.0),
        peer_sample("10.1.2.2", 1.0),
        peer_sample("10.1.7.2", 1.0),
        peer_sample("10.1.99.2", 5.0),
    ];

    let rows = join("srl1", &device.interfaces, &device.sessions, Some(&samples));

    assert_eq!(rows.len(), 4);
    let first = &rows[0];
    assert_eq!(first.name, "ethernet-1/1");
    assert_eq!(first.peer, "10.1.2.2");
    assert_eq!(first.bgp, "up");
    assert_eq!((first.admin.as_str(), first.oper.as_str()), ("up", "up"));
    assert!(!first.differs);
    assert_eq!(rows[1].peer, "10.1.7.2");
    assert!(!rows[1].differs);
    let broken = &rows[2];
    assert_eq!(broken.peer, "");
    assert_eq!(broken.bgp, "");
    assert_eq!(broken.admin, "up");
    assert_eq!(broken.oper, "down");
    assert!(broken.differs);
    let loopback = &rows[3];
    assert_eq!(loopback.name, "lo0");
    assert_eq!(loopback.admin, "—");
    assert_eq!(loopback.oper, "—");
    assert_eq!(loopback.peer, "");
    assert!(!loopback.differs);

    let unknown = join("srl1", &device.interfaces, &device.sessions, None);

    assert!(unknown
        .iter()
        .all(|row| row.admin == "?" && row.oper == "?"));
    assert_eq!(unknown[0].bgp, "?");
    assert!(unknown.iter().all(|row| !row.differs));
    assert_eq!(unknown[2].bgp, "");

    let states = oper_states("srl1", &device.sessions, &samples);
    assert_eq!(
        states,
        values(&[("10.1.2.2", 1.0), ("10.1.7.2", 1.0), ("10.1.99.2", 5.0)])
    );
    assert!(oper_states("srl2", &device.sessions, &samples).is_empty());

    let without_peer: Vec<Sample> = samples
        .iter()
        .filter(|sample| sample.labels.get("peer_address").map(String::as_str) != Some("10.1.2.2"))
        .cloned()
        .collect();
    let missing = join(
        "srl1",
        &device.interfaces,
        &device.sessions,
        Some(&without_peer),
    );
    assert_eq!(missing[0].peer, "10.1.2.2");
    assert_eq!(missing[0].bgp, "—");
    assert!(!missing[0].differs);
}

#[test]
fn the_merge_orders_four_kinds_newest_first_cuts_the_window_and_counts_them() {
    let now = UNIX_EPOCH + Duration::from_secs(NOW_SECONDS);
    let alerts = [
        alert("srl1", "warning", "active", false, "2026-11-09T13:53:32.5Z"),
        alert(
            "srl1",
            "warning",
            "suppressed",
            true,
            "2026-11-09T13:51:06Z",
        ),
        alert("srl2", "warning", "active", false, "2026-11-09T13:53:40Z"),
    ];
    let runs = [
        run(
            "evidence | srl1:10.1.99.2",
            "srl1",
            "2026-11-09T13:49:46.646183Z",
        ),
        run(
            "policy | srl1:10.1.99.2",
            "srl1",
            "2026-11-09T13:49:47.741124Z",
        ),
        run(
            "action | srl1:10.1.99.2",
            "srl1",
            "2026-11-09T13:49:48.867606Z",
        ),
        run(
            "action | srl2:10.1.11.1",
            "srl2",
            "2026-11-09T13:49:48.872384Z",
        ),
    ];
    let rca_text = r#"{"timestamp":"2026-11-09T13:49:48.890Z","severity":"info","message":"AI RCA:\nAI RCA disabled (set ENABLE_AI_RCA=true).","labels":{"ai_rca":"true","device":"srl1","peer_address":"10.1.99.2","source":"prefect"},"fields":{}}"#;
    let rcas = [Line {
        nanoseconds: nanoseconds("2026-11-09T13:49:48Z") + 890_000_000,
        text: rca_text.to_string(),
    }];
    let logs = [
        (nanoseconds("2026-11-09T13:54:50Z"), "newest nested"),
        (nanoseconds("2026-11-09T13:53:00Z"), "second nested"),
        (
            nanoseconds("2026-11-09T13:52:10Z"),
            "Interface ethernet-1/11 changed to down",
        ),
        (nanoseconds("2026-11-09T13:51:00Z"), "third nested"),
        (
            nanoseconds("2026-11-09T13:50:30Z"),
            "level=info plain text line",
        ),
        (nanoseconds("2026-11-09T13:20:00Z"), "older than the window"),
    ];

    let entries = merge(
        "srl1",
        &alerts,
        &runs,
        &rcas,
        &logs,
        Duration::from_secs(30 * 60),
        now,
    );

    let seen: Vec<(Kind, &str)> = entries
        .iter()
        .map(|entry| (entry.kind, entry.text.lines().next().unwrap()))
        .collect();
    assert_eq!(
        seen,
        [
            (Kind::Log, "newest nested"),
            (Kind::Alert, "BgpSessionNotUp  firing"),
            (Kind::Log, "second nested"),
            (Kind::Log, "Interface ethernet-1/11 changed to down"),
            (Kind::Alert, "BgpSessionNotUp  silenced"),
            (Kind::Log, "third nested"),
            (Kind::Log, "level=info plain text line"),
            (Kind::Rca, "AI RCA disabled (set ENABLE_AI_RCA=true)."),
            (Kind::Flow, "action | srl1:10.1.99.2  COMPLETED"),
            (Kind::Flow, "policy | srl1:10.1.99.2  COMPLETED"),
            (Kind::Flow, "evidence | srl1:10.1.99.2  COMPLETED"),
        ]
    );
    assert!(entries
        .windows(2)
        .all(|pair| pair[0].nanoseconds >= pair[1].nanoseconds));
    assert_eq!(counts(&entries), [2, 3, 1, 5]);
    assert_eq!(
        entries[1].text,
        "BgpSessionNotUp  firing\nalertname=BgpSessionNotUp\ndevice=srl1\nseverity=warning"
    );
    assert!(entries[7].text.starts_with("AI RCA disabled"));
}

const PROMETHEUS_BODY: &str = r#"{"status":"success","data":{"resultType":"vector","result":[
    {"metric":{"__name__":"bgp_oper_state","device":"srl1","peer_address":"10.1.2.2"},"value":[1793960400,"1"]},
    {"metric":{"__name__":"bgp_oper_state","device":"srl1","peer_address":"10.1.7.2"},"value":[1793960400,"2"]}
]}}"#;
const ALERTS_BODY: &str = r#"[{"labels":{"alertname":"BgpSessionNotUp","severity":"critical","device":"srl1"},"status":{"state":"active","silencedBy":[],"inhibitedBy":[]},"startsAt":"2026-11-09T13:40:00.000Z"}]"#;
const LOKI_BODY: &str = r#"{"status":"success","data":{"resultType":"streams","result":[
    {"stream":{"device":"srl1"},"values":[["1793960390000000000","a-new"],["1793960300000000000","a-old"]]}
]}}"#;
const PREFECT_BODY: &str = r#"[{"id":"q1","name":"quarantine_bgp | srl1:10.1.99.2","parameters":{"device":"srl1","peer_address":"10.1.99.2"},"start_time":"2026-11-09T13:41:00Z","state":{"type":"COMPLETED"},"parent_task_run_id":null}]"#;

struct Silent;

impl Runner for Silent {
    fn run(&self, _program: &str, _args: &[OsString]) -> io::Result<i32> {
        Ok(0)
    }

    fn output(&self, _program: &str, _args: &[OsString]) -> io::Result<Output> {
        Ok(Output {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

#[test]
fn the_first_answers_of_the_five_device_pollers_reach_the_screen() {
    let stub = Stub::start(&[
        ("GET", "/api/v1/query*", 200, PROMETHEUS_BODY),
        ("GET", "/api/v2/alerts*", 200, ALERTS_BODY),
        ("GET", "/loki/api/v1/query_range*", 200, LOKI_BODY),
        ("POST", "/api/flow_runs/filter", 200, PREFECT_BODY),
    ]);
    let address = || stub.url().to_string();
    let urls = Urls {
        grafana: address(),
        prometheus: address(),
        loki: address(),
        alertmanager: address(),
        infrahub: address(),
        prefect: address(),
        sonda: address(),
    };
    let (pollers, updates) = Pollers::start(
        Arc::new(Silent),
        Vec::new(),
        0,
        SondaTarget {
            server: None,
            api_key: None,
        },
        Some(Arc::new(fixed(WORKSHOP_ONE))),
        Some(urls),
    );
    pollers.send(Command::SelectDevice("srl1".to_string()));
    pollers.send(Command::OpenTab(Tab::Timeline));

    let mut devices = None;
    let mut metrics = None;
    let mut alerts = None;
    let mut logs = None;
    let mut rcas = None;
    let mut runs = None;
    let end = Instant::now() + Duration::from_secs(5);
    while [
        devices.is_some(),
        metrics.is_some(),
        alerts.is_some(),
        logs.is_some(),
        rcas.is_some(),
        runs.is_some(),
    ]
    .contains(&false)
    {
        let update = updates
            .recv_timeout(end.saturating_duration_since(Instant::now()))
            .expect("the six answers in time");
        match update {
            Update::Devices(Ok(list)) => devices = Some(list),
            Update::Metrics(Ok(list)) => metrics = Some(list),
            Update::Alerts(Ok(list)) => alerts = Some(list),
            Update::DeviceLogs(name, Ok(list)) if name == "srl1" => logs = Some(list),
            Update::Rcas(name, Ok(list)) if name == "srl1" => rcas = Some(list),
            Update::FlowRuns(Ok(list)) => runs = Some(list),
            _ => {}
        }
    }
    pollers.send(Command::Quit);

    assert!(
        stub.requests().iter().any(|request| request
            .path
            .contains("query=%7Bdevice%3D%22srl1%22%2C%20source!%3D%22prefect%22%7D")),
        "{:?}",
        stub.requests()
    );
    let names: Vec<String> = devices.unwrap().into_iter().map(|d| d.name).collect();
    assert_eq!(names, ["srl1", "srl2"]);
    assert_eq!(metrics.unwrap().len(), 2);
    let alerts = alerts.unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].labels["alertname"], "BgpSessionNotUp");
    assert_eq!(logs.unwrap().len(), 2);
    assert_eq!(rcas.unwrap().len(), 2);
    let runs = runs.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].device.as_deref(), Some("srl1"));
}

#[test]
fn the_services_follow_does_not_stop_the_device_logs_and_the_device_follow_does() {
    let stub = Stub::start(&[
        ("GET", "/api/v1/query*", 200, PROMETHEUS_BODY),
        ("GET", "/api/v2/alerts*", 200, ALERTS_BODY),
        ("GET", "/loki/api/v1/query_range*", 200, LOKI_BODY),
        ("POST", "/api/flow_runs/filter", 200, PREFECT_BODY),
    ]);
    let address = || stub.url().to_string();
    let urls = Urls {
        grafana: address(),
        prometheus: address(),
        loki: address(),
        alertmanager: address(),
        infrahub: address(),
        prefect: address(),
        sonda: address(),
    };
    let (pollers, updates) = Pollers::start(
        Arc::new(Silent),
        Vec::new(),
        0,
        SondaTarget {
            server: None,
            api_key: None,
        },
        Some(Arc::new(fixed(WORKSHOP_ONE))),
        Some(urls),
    );
    let device_logs_within = |wait: Duration| {
        let end = Instant::now() + wait;
        loop {
            match updates.recv_timeout(end.saturating_duration_since(Instant::now())) {
                Ok(Update::DeviceLogs(..)) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    };
    pollers.send(Command::SelectDevice("srl1".to_string()));
    pollers.send(Command::OpenTab(Tab::Logs));
    assert!(device_logs_within(Duration::from_secs(5)));

    pollers.send(Command::Follow(false));
    pollers.send(Command::Refresh);
    assert!(
        device_logs_within(Duration::from_secs(3)),
        "the services follow command paused the device logs"
    );

    pollers.send(Command::FollowDevice(false));
    pollers.send(Command::Refresh);
    let polled = device_logs_within(Duration::from_millis(500));
    pollers.send(Command::Quit);

    assert!(
        !polled,
        "the device follow command did not pause the device logs"
    );
}
