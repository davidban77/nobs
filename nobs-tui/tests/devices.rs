use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, UNIX_EPOCH};

use nobs_core::alertmanager::Alert;
use nobs_core::lab_vars;
use nobs_core::loki::Line;
use nobs_core::prefect::FlowRun;
use nobs_core::prometheus::Sample;
use nobs_core::sot::{Device, Fixed, SourceOfTruth};
use nobs_core::time;
use nobs_tui::devices::{
    alert_count, alert_state, counts, health, join, list_rows, merge, oper_states, Health, Kind,
};

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

fn line(time_text: &str, text: &str) -> Line {
    Line {
        nanoseconds: nanoseconds(time_text),
        text: text.to_string(),
    }
}

#[test]
fn the_list_rows_follow_the_source_order_with_the_marker_the_dash_kind_and_the_sync_dash() {
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

    let ok = health(&sessions, Some(&all_up));
    let not_up = health(&sessions, Some(&one_active));
    let missing = health(&sessions, Some(&one_missing));
    let down = health(&sessions, Some(&BTreeMap::new()));
    let unknown = health(&sessions, None);
    let none = health(&[], Some(&all_up));

    assert_eq!(ok, Health::Ok);
    assert_eq!(not_up, Health::Warn);
    assert_eq!(missing, Health::Warn);
    assert_eq!(down, Health::Down);
    assert_eq!(unknown, Health::Unknown);
    assert_eq!(none, Health::NoSessions);
    let words: Vec<&str> = [ok, not_up, missing, down, unknown, none]
        .into_iter()
        .map(Health::text)
        .collect();
    assert_eq!(words, ["ok", "warn", "warn", "down", "?", "—"]);
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
    assert_eq!(alert_state(&alerts[3]), "silenced");
    assert_eq!(alert_state(&alerts[4]), "silenced");
    assert_eq!(alert_state(&alerts[0]), "firing");

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
    assert_eq!(unknown[2].bgp, "");

    let states = oper_states("srl1", &samples);
    assert_eq!(
        states,
        values(&[("10.1.2.2", 1.0), ("10.1.7.2", 1.0), ("10.1.99.2", 5.0)])
    );
    assert!(oper_states("srl2", &samples).is_empty());
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
    let nested = |message: &str| {
        format!(
            r#"{{"timestamp":"2026-11-09T13:51:17.170Z","severity":"info","message":"{message}","labels":{{"device":"srl1"}},"fields":{{}}}}"#
        )
    };
    let flat = r#"{"message":"Interface ethernet-1/11 changed to down","level":"err","hostname":"srl1","sonda":{"scenario":"flap"}}"#;
    let logs = [
        line("2026-11-09T13:54:50Z", &nested("newest nested")),
        line("2026-11-09T13:53:00Z", &nested("second nested")),
        line("2026-11-09T13:52:10Z", flat),
        line("2026-11-09T13:51:00Z", &nested("third nested")),
        line("2026-11-09T13:50:30Z", "level=info plain text line"),
        line("2026-11-09T13:20:00Z", "older than the window"),
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
