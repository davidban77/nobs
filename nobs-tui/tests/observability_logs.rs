use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use nobs_core::lab::services;
use nobs_core::status::Service;
use nobs_tui::{Command, Tab, Update, Workbench};

#[path = "../../tests/support/stub.rs"]
mod stub;
use stub::Stub;

mod pollers;
use pollers::{press, pump_until, send, shell, start_pollers};

const LAB_QUERY_SENT: &str = "query=%7Bdevice%3D~%22.%2B%22%2C%20ai_rca!%3D%22true%22%7D&";
const LOKI_BODY: &str = r#"{"status":"success","data":{"resultType":"streams","result":[
    {"stream":{"device":"srl1"},"values":[["1793960390000000000","a line of srl1"]]},
    {"stream":{"job":"lab"},"values":[["1793960380000000000","a line with no device"]]}
]}}"#;
const QUIET: Duration = Duration::from_millis(2500);

fn loki() -> Stub {
    Stub::start(&[("GET", "/loki/api/v1/query_range*", 200, LOKI_BODY)])
}

fn lab_requests(stub: &Stub) -> Vec<String> {
    stub.requests()
        .into_iter()
        .map(|request| request.path)
        .filter(|path| path.contains(LAB_QUERY_SENT))
        .collect()
}

fn listed(logs: bool) -> Update {
    let (defined, rows) = if logs {
        (
            vec!["loki".to_string()],
            r#"{"Service":"loki","State":"running"}"#,
        )
    } else {
        (Vec::new(), "")
    };
    Update::Services(0, Ok(defined), Ok(services(rows).unwrap()))
}

fn wait_for(
    updates: &Receiver<Update>,
    wait: Duration,
    wanted: impl Fn(&Update) -> bool,
) -> Option<Update> {
    let end = Instant::now() + wait;
    while let Ok(update) = updates.recv_timeout(end.saturating_duration_since(Instant::now())) {
        if wanted(&update) {
            return Some(update);
        }
    }
    None
}

fn lab_logs(update: &Update) -> bool {
    matches!(update, Update::LabLogs(_))
}

fn device_logs(update: &Update) -> bool {
    matches!(update, Update::DeviceLogs(..))
}

fn lab_requests_of_a_shell(home: Workbench, key: Option<char>, logs: bool) -> Vec<String> {
    let stub = loki();
    let (pollers, updates) = start_pollers(&stub);
    let mut model = shell(home);
    if let Some(typed) = key {
        send(&pollers, press(&mut model, typed));
    }
    let asked = model.apply(listed(logs));
    assert_eq!(asked.contains(&Command::Ask(Service::Logs, true)), logs);
    send(&pollers, asked);
    pump_until(&mut model, &pollers, &updates, QUIET, |_| false);
    pollers.send(Command::Quit);
    lab_requests(&stub)
}

fn lab_requests_shown_then_left(keys: &[char]) -> (usize, usize, usize) {
    let stub = loki();
    let (pollers, updates) = start_pollers(&stub);
    let mut model = shell(Workbench::Devices);
    for typed in ['3', ']'] {
        send(&pollers, press(&mut model, typed));
    }
    send(&pollers, model.apply(listed(true)));
    pump_until(&mut model, &pollers, &updates, QUIET, |_| false);
    let shown = lab_requests(&stub).len();

    for typed in keys {
        send(&pollers, press(&mut model, *typed));
    }
    send(&pollers, model.apply(listed(true)));
    pump_until(
        &mut model,
        &pollers,
        &updates,
        Duration::from_millis(300),
        |_| false,
    );
    let left = lab_requests(&stub).len();
    pump_until(&mut model, &pollers, &updates, QUIET, |_| false);
    pollers.send(Command::Quit);
    (shown, left, lab_requests(&stub).len())
}

#[test]
fn leaving_the_logs_view_for_another_workbench_stops_the_request_for_the_lines_of_all_devices() {
    for key in ['1', '2'] {
        let (shown, left, later) = lab_requests_shown_then_left(&[key]);

        assert!(shown > 0, "{key}");
        assert_eq!(later, left, "{key}");
    }
}

#[test]
fn going_back_to_the_alerts_view_stops_the_request_for_the_lines_of_all_devices() {
    let (shown, left, later) = lab_requests_shown_then_left(&['[']);

    assert!(shown > 0);
    assert_eq!(later, left);
}

#[test]
fn the_alerts_view_sends_no_request_for_the_lines_of_all_devices() {
    let sent = lab_requests_of_a_shell(Workbench::Devices, Some('3'), true);

    assert_eq!(sent, Vec::<String>::new());
}

#[test]
fn another_workbench_sends_no_request_for_the_lines_of_all_devices() {
    for home in [Workbench::Devices, Workbench::Services] {
        let sent = lab_requests_of_a_shell(home, None, true);

        assert_eq!(sent, Vec::<String>::new(), "{home:?}");
    }
}

#[test]
fn a_lab_without_the_logs_service_sends_no_request_for_the_lines_of_all_devices() {
    let stub = loki();
    let (pollers, updates) = start_pollers(&stub);
    let mut model = shell(Workbench::Devices);
    send(&pollers, press(&mut model, '3'));
    let asked = model.apply(listed(false));
    assert!(
        !asked.contains(&Command::Ask(Service::Logs, true)),
        "{asked:?}"
    );
    send(&pollers, asked);

    pollers.send(Command::FollowLab(true));
    pump_until(&mut model, &pollers, &updates, QUIET, |_| false);
    pollers.send(Command::Quit);

    assert_eq!(lab_requests(&stub), Vec::<String>::new());
}

#[test]
fn a_paused_logs_view_sends_no_request_for_the_lines_of_all_devices() {
    let (shown, paused, later) = lab_requests_shown_then_left(&['f']);

    assert!(shown > 0);
    assert_eq!(later, paused);
}

#[test]
fn the_shown_and_following_logs_view_asks_for_the_lines_of_all_devices_with_a_limit_of_200() {
    let stub = loki();
    let (pollers, updates) = start_pollers(&stub);
    pollers.send(Command::Ask(Service::Logs, true));
    pollers.send(Command::FollowLab(true));

    let answer = wait_for(&updates, Duration::from_secs(5), lab_logs);
    pollers.send(Command::Quit);

    let Some(Update::LabLogs(Ok(lines))) = answer else {
        panic!("no lines of all devices: {answer:?}");
    };
    let devices: Vec<Option<&str>> = lines
        .iter()
        .map(|(labels, _)| labels.get("device").map(String::as_str))
        .collect();
    assert_eq!(devices, [Some("srl1"), None]);
    let sent = lab_requests(&stub);
    assert!(!sent.is_empty(), "{:?}", stub.requests());
    assert!(
        sent.iter().all(|path| path.contains("&limit=200&")),
        "{sent:?}"
    );
}

#[test]
fn the_device_follow_and_the_lab_follow_do_not_stop_each_other() {
    let stub = loki();
    let (pollers, updates) = start_pollers(&stub);
    pollers.send(Command::SelectDevice("srl1".to_string()));
    pollers.send(Command::OpenTab(Tab::Logs));
    pollers.send(Command::Ask(Service::Logs, true));
    pollers.send(Command::FollowLab(true));
    assert!(wait_for(&updates, Duration::from_secs(5), lab_logs).is_some());

    pollers.send(Command::FollowLab(false));
    let device_kept = wait_for(&updates, Duration::from_secs(4), device_logs).is_some();

    pollers.send(Command::FollowLab(true));
    assert!(wait_for(&updates, Duration::from_secs(5), lab_logs).is_some());
    pollers.send(Command::FollowDevice(false));
    let lab_kept = wait_for(&updates, Duration::from_secs(4), lab_logs).is_some();
    pollers.send(Command::Quit);

    assert!(device_kept, "the lab follow stopped the device logs");
    assert!(
        lab_kept,
        "the device follow stopped the lines of all devices"
    );
}
