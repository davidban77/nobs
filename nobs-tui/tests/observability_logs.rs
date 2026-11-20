use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nobs_core::lab::services;
use nobs_core::lab_vars;
use nobs_core::locate::{Kind, Located};
use nobs_core::runner::{Output, Runner};
use nobs_core::sot::Fixed;
use nobs_core::status::{Service, Urls};
use nobs_tui::{
    handle, Command, Pollers, Shell, SondaTarget, Stack, StackKind, Start, Tab, Update, Workbench,
};

#[path = "../../tests/support/stub.rs"]
mod stub;
use stub::Stub;

const WORKSHOP_ONE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/fixtures/workshop-one/lab_vars.yml"
);
const LAB_QUERY_SENT: &str = "query=%7Bdevice%3D~%22.%2B%22%2C%20ai_rca!%3D%22true%22%7D&";
const LOKI_BODY: &str = r#"{"status":"success","data":{"resultType":"streams","result":[
    {"stream":{"device":"srl1"},"values":[["1793960390000000000","a line of srl1"]]},
    {"stream":{"job":"lab"},"values":[["1793960380000000000","a line with no device"]]}
]}}"#;
const QUIET: Duration = Duration::from_millis(2500);

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

fn loki() -> Stub {
    Stub::start(&[("GET", "/loki/api/v1/query_range*", 200, LOKI_BODY)])
}

fn start_pollers(stub: &Stub) -> (Pollers, Receiver<Update>) {
    let address = || stub.url().to_string();
    let urls = Urls {
        grafana: address(),
        prometheus: address(),
        loki: address(),
        alertmanager: address(),
        infrahub: address(),
        prefect: address(),
        sonda: address(),
        set: Vec::new(),
    };
    let text = fs::read_to_string(WORKSHOP_ONE).unwrap();
    let source = Fixed::from_lab_vars(&lab_vars::read(&text).unwrap());
    Pollers::start(
        Arc::new(Silent),
        Vec::new(),
        0,
        SondaTarget {
            server: None,
            api_key: None,
        },
        Some((Arc::new(source), None)),
        Some(urls),
        None,
    )
}

fn lab_requests(stub: &Stub) -> Vec<String> {
    stub.requests()
        .into_iter()
        .map(|request| request.path)
        .filter(|path| path.contains(LAB_QUERY_SENT))
        .collect()
}

fn shell(home: Workbench) -> Shell {
    let path = PathBuf::from("/ws");
    Shell::new(Start {
        lab: Some(Located {
            dir: path.clone(),
            kind: Kind::LabcliLab(None),
        }),
        stacks: vec![Stack {
            name: "ws".to_string(),
            kind: StackKind::Workshop,
            compose: path.join("docker-compose.yml"),
            path,
            project: None,
            env: Vec::new(),
            shows_sonda: false,
        }],
        sonda: SondaTarget {
            server: None,
            api_key: None,
        },
        home,
        chooser: false,
        source: None,
        source_error: None,
        source_service: None,
        urls: Urls::defaults(),
        program: PathBuf::from("nobs"),
        login: Arc::new(|_: &str| None),
    })
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

fn send(pollers: &Pollers, commands: Vec<Command>) {
    for command in commands {
        pollers.send(command);
    }
}

fn pump(model: &mut Shell, pollers: &Pollers, updates: &Receiver<Update>, wait: Duration) {
    let end = Instant::now() + wait;
    while let Ok(update) = updates.recv_timeout(end.saturating_duration_since(Instant::now())) {
        send(pollers, model.apply(update));
    }
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
        send(
            &pollers,
            handle(
                &mut model,
                KeyEvent::new(KeyCode::Char(typed), KeyModifiers::NONE),
            ),
        );
    }
    let asked = model.apply(listed(logs));
    assert_eq!(asked.contains(&Command::Ask(Service::Logs, true)), logs);
    send(&pollers, asked);
    pump(&mut model, &pollers, &updates, QUIET);
    pollers.send(Command::Quit);
    lab_requests(&stub)
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
    send(
        &pollers,
        handle(
            &mut model,
            KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE),
        ),
    );
    let asked = model.apply(listed(false));
    assert!(
        !asked.contains(&Command::Ask(Service::Logs, true)),
        "{asked:?}"
    );
    send(&pollers, asked);

    pollers.send(Command::FollowLab(true));
    pump(&mut model, &pollers, &updates, QUIET);
    pollers.send(Command::Quit);

    assert_eq!(lab_requests(&stub), Vec::<String>::new());
}

#[test]
fn a_paused_logs_view_sends_no_request_for_the_lines_of_all_devices() {
    let stub = loki();
    let (pollers, updates) = start_pollers(&stub);
    pollers.send(Command::Ask(Service::Logs, true));
    pollers.send(Command::FollowLab(true));
    assert!(wait_for(&updates, Duration::from_secs(5), lab_logs).is_some());
    let before = lab_requests(&stub).len();

    pollers.send(Command::FollowLab(false));
    pollers.send(Command::Refresh);
    let answered = wait_for(&updates, QUIET, lab_logs);
    pollers.send(Command::Quit);

    assert!(answered.is_none());
    assert_eq!(lab_requests(&stub).len(), before);
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
