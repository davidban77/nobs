use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nobs_core::alertmanager::Alert;
use nobs_core::lab::{services, Service};
use nobs_core::lab_vars;
use nobs_core::locate::{locate, Kind, Located};
use nobs_core::loki::Line as LokiLine;
use nobs_core::prefect::FlowRun;
use nobs_core::prometheus::Sample;
use nobs_core::sot::{Fixed, SourceOfTruth};
use nobs_core::status::Urls;
use nobs_tui::book::NodeRow;
use nobs_tui::shell::{stacks, Health};
use nobs_tui::{
    data, draw, handle, view, End, Shell, SondaTarget, Stack, StackKind, Start, Update, Workbench,
};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

const CLOCK_SECONDS: u64 = 1_794_231_727;
const WORKSHOP_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/fixtures/workshop-one"
);
const WORKSHOP_ONE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/fixtures/workshop-one/lab_vars.yml"
);
const BOOK_LAB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/book-lab");
const LAB_DIR: &str = "/home/me/book-lab";
const LONG_LAB_DIR: &str = "/home/me/labs/network-observability/book-lab";
const ALREADY_EXIST: &str = "nobs: lab deploy: node containers already exist (clab-nobs-t-book-nobs-t-srl1, clab-nobs-t-book-nobs-t-srl2); run nobs lab destroy first";
const LONG_DIR: &str = "/Users/netpanda/projects/nobs/target/slice-21-workshops/workshops/packt";
const SIZES: [(u16, u16); 2] = [(60, 15), (100, 30)];
const WORKSHOP: [(&str, &str, u16, &str); 7] = [
    ("grafana", "running", 3000, "41 seconds ago"),
    ("prometheus", "running", 9090, "41 seconds ago"),
    ("loki", "running", 3001, "40 seconds ago"),
    ("alertmanager", "running", 9093, "40 seconds ago"),
    ("infrahub", "running", 8000, "38 seconds ago"),
    ("prefect", "running", 4200, "37 seconds ago"),
    ("sonda", "running", 8085, "36 seconds ago"),
];
const DEMO: [(&str, &str, u16, &str); 3] = [
    ("web", "exited", 0, "2 days ago"),
    ("db", "exited", 0, "2 days ago"),
    ("cache", "exited", 0, "2 days ago"),
];
const LOG_LINES: [&str; 4] = [
    r#"level=warn ts=13:41:58 msg="ingester not ready""#,
    r#"level=info ts=13:41:59 msg="ring state JOINING""#,
    r#"level=info ts=13:42:01 msg="ring state ACTIVE""#,
    r#"level=info ts=13:42:01 msg="Loki started""#,
];
const SCENARIOS: &str = r#"{"scenarios":[{"id":"a1b2c3d4e5","name":"cpu","state":"running","elapsed_secs":12,"degraded":false}]}"#;

fn stack(name: &str, env: &[(&str, &str)]) -> Stack {
    let path = PathBuf::from("/home/me/ws").join(name);
    Stack {
        name: name.to_string(),
        kind: StackKind::Workshop,
        compose: path.join("docker-compose.yml"),
        path,
        project: Some(name.to_string()),
        env: env
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    }
}

fn ps(rows: &[(&str, &str, u16, &str)]) -> Vec<Service> {
    let text: Vec<String> = rows
        .iter()
        .map(|(name, state, port, age)| {
            format!(
                r#"{{"Service":"{name}","State":"{state}","Publishers":[{{"PublishedPort":{port}}}],"RunningFor":"{age}"}}"#
            )
        })
        .collect();
    services(&text.join("\n")).unwrap()
}

fn health() -> Vec<Health> {
    WORKSHOP
        .iter()
        .map(|(name, ..)| Health {
            service: name.to_string(),
            state: match *name {
                "loki" => "down",
                "infrahub" => "http 503",
                _ => "ok",
            }
            .to_string(),
        })
        .collect()
}

fn press(model: &mut Shell, code: KeyCode) {
    handle(model, KeyEvent::new(code, KeyModifiers::NONE));
}

fn sonda_only() -> Stack {
    let path = PathBuf::from("/home/me");
    Stack {
        name: "sonda only".to_string(),
        kind: StackKind::Sonda,
        compose: path.clone(),
        path,
        project: None,
        env: vec![(
            "SONDA_SERVER_URL".to_string(),
            "http://localhost:8080".to_string(),
        )],
    }
}

fn clock() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(CLOCK_SECONDS)
}

fn begin(lab: Option<Located>, stacks: Vec<Stack>, home: Workbench) -> Start {
    Start {
        lab,
        stacks,
        sonda: SondaTarget {
            server: None,
            api_key: None,
        },
        home,
        chooser: true,
        source: None,
        source_error: None,
        urls: Urls::defaults(),
        program: PathBuf::from("nobs"),
    }
}

fn model(chooser: bool, packt_failed: bool) -> Shell {
    model_with(chooser, packt_failed, None, None)
}

fn model_with(
    chooser: bool,
    packt_failed: bool,
    source: Option<Arc<dyn SourceOfTruth>>,
    lab: Option<Located>,
) -> Shell {
    let start = Instant::now();
    let mut stacks = vec![
        stack("autocon5", &[("INFRAHUB_ADDRESS", "http://localhost:8000")]),
        stack("packt", &[]),
        stack("demo", &[]),
    ];
    if chooser {
        stacks.push(sonda_only());
    }
    let mut model = Shell::new(Start {
        source,
        ..begin(lab, stacks, Workbench::Services)
    });
    if !chooser {
        press(&mut model, KeyCode::Enter);
    }
    model.tick(start, clock(), (80, 24));
    if packt_failed {
        model.apply(Update::Services(1, Err("exit 1".to_string())));
        model.tick(start + Duration::from_secs(10), clock(), (80, 24));
    }
    model.apply(Update::Services(0, Ok(ps(&WORKSHOP))));
    model.apply(Update::Health(0, health()));
    if !packt_failed {
        model.apply(Update::Services(1, Ok(Vec::new())));
    }
    model.apply(Update::Services(2, Ok(ps(&DEMO))));
    model
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

fn metrics() -> Vec<Sample> {
    let mut samples = Vec::new();
    let peers = [
        ("srl1", "10.1.2.2", 1.0),
        ("srl1", "10.1.7.2", 1.0),
        ("srl1", "10.1.99.2", 5.0),
        ("srl2", "10.1.2.1", 1.0),
        ("srl2", "10.1.7.1", 1.0),
        ("srl2", "10.1.11.1", 1.0),
    ];
    for (device, peer, value) in peers {
        samples.push(sample(
            &[
                ("__name__", "bgp_oper_state"),
                ("device", device),
                ("peer_address", peer),
                ("afi_safi_name", "ipv4-unicast"),
                ("name", "default"),
            ],
            value,
        ));
    }
    for device in ["srl1", "srl2"] {
        for (name, oper) in [
            ("ethernet-1/1", 1.0),
            ("ethernet-1/10", 1.0),
            ("ethernet-1/11", 2.0),
        ] {
            for (metric, value) in [
                ("interface_admin_state", 1.0),
                ("interface_oper_state", oper),
            ] {
                samples.push(sample(
                    &[("__name__", metric), ("device", device), ("name", name)],
                    value,
                ));
            }
        }
    }
    samples
}

fn alert(state: &str, silenced: bool, peer: &str, starts_at: &str) -> Alert {
    Alert {
        labels: labels(&[
            ("alertname", "BgpSessionNotUp"),
            ("device", "srl1"),
            ("peer_address", peer),
            ("severity", "warning"),
        ]),
        state: state.to_string(),
        silenced,
        inhibited: false,
        starts_at: starts_at.to_string(),
        silenced_by: Vec::new(),
        annotations: BTreeMap::new(),
    }
}

fn alerts() -> Vec<Alert> {
    vec![
        alert("active", false, "10.1.99.2", "2026-11-09T13:41:46Z"),
        alert("suppressed", true, "10.1.2.2", "2026-11-09T13:40:35Z"),
    ]
}

fn nanoseconds(seconds_before_clock: u64, extra: i64) -> i64 {
    i64::try_from((CLOCK_SECONDS - seconds_before_clock) * 1_000_000_000).unwrap() + extra
}

fn loki_line(seconds_before_clock: u64, extra: i64, text: &str) -> LokiLine {
    LokiLine {
        nanoseconds: nanoseconds(seconds_before_clock, extra),
        text: text.to_string(),
    }
}

fn device_logs() -> Vec<LokiLine> {
    vec![
        loki_line(
            50,
            500_000_000,
            "chassis|1044|8|00012|Temperature sensor cpu: temperature is normal",
        ),
        loki_line(
            50,
            0,
            r#"{"message":"BGP neighbor 10.1.7.2: state changed to idle"}"#,
        ),
        loki_line(
            60,
            0,
            r#"{"message":"BGP neighbor 10.1.99.2: connection refused — retrying in 30 seconds","source":{"host":"srl1"}}"#,
        ),
        loki_line(
            70,
            0,
            r#"{"message":"BGP neighbor 10.1.7.2: state changed to active","source":{"host":"srl1"}}"#,
        ),
        loki_line(
            100,
            0,
            r#"{"message":"BGP neighbor 10.1.99.2: TCP session establishment failed","source":{"host":"srl1"}}"#,
        ),
    ]
}

fn rcas() -> Vec<LokiLine> {
    vec![loki_line(
        79,
        500_000_000,
        r#"{"timestamp":"2026-11-09T13:40:48.500Z","severity":"info","message":"AI RCA:\nAI RCA disabled (set ENABLE_AI_RCA=true to enable the analysis).","labels":{"ai_rca":"true","device":"srl1","peer_address":"10.1.99.2","source":"prefect"},"fields":{}}"#,
    )]
}

fn flow_runs() -> Vec<FlowRun> {
    [
        ("evidence", "2026-11-09T13:40:46Z"),
        ("policy", "2026-11-09T13:40:47Z"),
        ("action", "2026-11-09T13:40:48Z"),
    ]
    .into_iter()
    .map(|(name, start_time)| FlowRun {
        id: format!("id-{name}"),
        name: format!("{name} | srl1:10.1.99.2"),
        device: Some("srl1".to_string()),
        peer: Some("10.1.99.2".to_string()),
        start_time: start_time.to_string(),
        state: "COMPLETED".to_string(),
        parent_task_run_id: None,
    })
    .collect()
}

fn devices_model() -> Shell {
    let source =
        Fixed::from_lab_vars(&lab_vars::read(&fs::read_to_string(WORKSHOP_ONE).unwrap()).unwrap());
    source.set_maintenance("srl2", true).unwrap();
    let devices = source.devices().unwrap();
    let mut model = model_with(false, false, Some(Arc::new(source)), None);
    press(&mut model, KeyCode::Char('1'));
    model.apply(Update::Devices(Ok(devices)));
    model.apply(Update::Metrics(Ok(metrics())));
    model.apply(Update::Alerts(Ok(alerts())));
    model.apply(Update::DeviceLogs("srl1".to_string(), Ok(device_logs())));
    model.apply(Update::Rcas("srl1".to_string(), Ok(rcas())));
    model.apply(Update::FlowRuns(Ok(flow_runs())));
    model
}

fn on_tab(presses: usize) -> Shell {
    let mut model = devices_model();
    press(&mut model, KeyCode::Enter);
    for _ in 0..presses {
        press(&mut model, KeyCode::Char(']'));
    }
    model
}

fn without_a_source() -> Shell {
    let dir = PathBuf::from(WORKSHOP_DIR);
    let mut lab = locate(&dir, Some(&dir)).unwrap();
    lab.dir = PathBuf::from("/home/me/ws/autocon5");
    if let Kind::Workshop(workshop) = &mut lab.kind {
        workshop.name = "autocon5".to_string();
        workshop.sot = "none".to_string();
    }
    let mut model = model_with(false, false, None, Some(lab));
    press(&mut model, KeyCode::Char('1'));
    model.apply(Update::Alerts(Ok(alerts())));
    model
}

fn logs_model(paused: bool) -> Shell {
    let mut model = model(false, false);
    press(&mut model, KeyCode::Tab);
    press(&mut model, KeyCode::Char('j'));
    press(&mut model, KeyCode::Char('j'));
    press(&mut model, KeyCode::Enter);
    let lines = LOG_LINES.iter().map(|line| format!("13:42:02 {line}"));
    model.apply(Update::Logs(0, "loki".to_string(), Ok(lines.collect())));
    if paused {
        press(&mut model, KeyCode::Char('f'));
    }
    model
}

fn sonda_model() -> Shell {
    let mut model = model(false, false);
    press(&mut model, KeyCode::Tab);
    press(&mut model, KeyCode::Char('['));
    let at = model.now();
    model.apply(Update::Sonda(data::Update {
        at,
        list: Ok(SCENARIOS.to_string()),
        selected: Some((
            "a1b2c3d4e5".to_string(),
            Ok(r#"{"name":"cpu"}"#.to_string()),
            Ok(r#"{"state":"running"}"#.to_string()),
        )),
    }));
    model
}

fn pressed(key: char) -> Shell {
    let mut model = model(false, false);
    press(&mut model, KeyCode::Char(key));
    model
}

fn node(name: &str, state: Option<&str>) -> NodeRow {
    NodeRow {
        name: name.to_string(),
        kind: "nokia_srlinux".to_string(),
        state: state.map(str::to_string),
    }
}

fn lab_model() -> Shell {
    lab_model_in(LAB_DIR)
}

fn lab_model_in(dir: &str) -> Shell {
    let mut model = unpolled_lab_model(dir);
    model.apply(Update::Nodes(Ok(vec![
        node("nobs-t-srl2", None),
        node("nobs-t-srl1", Some("running")),
    ])));
    model
}

fn unpolled_lab_model(lab_dir: &str) -> Shell {
    let dir = PathBuf::from(BOOK_LAB);
    let lab = locate(&dir, Some(&dir)).unwrap();
    let mut chapters = stacks(Some(&lab), Vec::new()).unwrap();
    for chapter in &mut chapters {
        chapter.path = PathBuf::from(lab_dir);
    }
    let mut model = Shell::new(Start {
        chooser: false,
        ..begin(Some(lab), chapters, Workbench::Lab)
    });
    model.tick(Instant::now(), clock(), (80, 24));
    model.apply(Update::Services(
        0,
        Ok(ps(&[("grafana", "running", 3000, "41 seconds ago")])),
    ));
    model.apply(Update::Services(1, Ok(Vec::new())));
    model
}

fn long_question() -> Shell {
    let packt = Stack {
        path: PathBuf::from(LONG_DIR),
        ..stack("nobs-t-packt", &[])
    };
    let mut model = Shell::new(begin(None, vec![packt], Workbench::Services));
    press(&mut model, KeyCode::Enter);
    model.tick(Instant::now(), clock(), (80, 24));
    press(&mut model, KeyCode::Char('u'));
    model
}

fn refused() -> Shell {
    refused_in(LAB_DIR)
}

fn refused_in(dir: &str) -> Shell {
    let mut model = lab_model_in(dir);
    press(&mut model, KeyCode::Char('D'));
    press(&mut model, KeyCode::Char('y'));
    model.apply(Update::ActionLine(ALREADY_EXIST.to_string()));
    later(&mut model, 2);
    model.apply(Update::ActionEnded(End::Exited(1)));
    model
}

fn later(model: &mut Shell, seconds: u64) {
    let now = model.now() + Duration::from_secs(seconds);
    model.tick(now, clock(), (80, 24));
}

fn deployed(end: End) -> Shell {
    let mut model = lab_model();
    press(&mut model, KeyCode::Char('D'));
    press(&mut model, KeyCode::Char('y'));
    later(&mut model, 41);
    model.apply(Update::ActionEnded(end));
    model
}

fn ran(end: Option<End>, folded: bool) -> Shell {
    let mut model = model(false, false);
    press(&mut model, KeyCode::Char('u'));
    press(&mut model, KeyCode::Char('y'));
    for line in [
        " Container autocon5-grafana-1  Starting",
        " Container autocon5-grafana-1  Started",
    ] {
        model.apply(Update::ActionLine(line.to_string()));
    }
    if let Some(end) = end {
        if end != End::Exited(0) {
            model.apply(Update::ActionLine(
                "nobs: up: compose up failed".to_string(),
            ));
        }
        later(&mut model, 41);
        model.apply(Update::ActionEnded(end));
    }
    if folded {
        press(&mut model, KeyCode::Esc);
    }
    model
}

fn not_stopped() -> Shell {
    let mut model = ran(None, false);
    handle(
        &mut model,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    );
    later(&mut model, 7);
    model.apply(Update::ActionEnded(End::NotStopped { group: 4242 }));
    model
}

fn pinned(model: Shell, name: &str, width: u16, height: u16) {
    let actual = lines(&render(model, width, height));

    let failure = mismatch(name, width, height, &actual);

    assert!(failure.is_none(), "{}", failure.unwrap_or_default());
}

fn row_has_no_green(buffer: &Buffer, y: u16) -> bool {
    (0..buffer.area.width).all(|x| buffer[(x, y)].fg != Color::Green)
}

fn render(mut model: Shell, width: u16, height: u16) -> Buffer {
    let now = model.now() + Duration::from_secs(2);
    model.tick(now, clock(), (width, height));
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame, &model)).unwrap();
    terminal.backend().buffer().clone()
}

fn lines(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let row: String = (0..area.width).map(|x| buffer[(x, y)].symbol()).collect();
            row.trim_end().to_string()
        })
        .collect()
}

fn mismatch(name: &str, width: u16, height: u16, actual: &[String]) -> Option<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/golden/{name}-{width}x{height}.txt"));
    let expected = fs::read_to_string(&path);
    let same = expected
        .as_ref()
        .is_ok_and(|text| text.lines().eq(actual.iter().map(String::as_str)));
    (!same).then(|| {
        format!(
            "{name} {width}x{height}\n--- expected ({})\n{}\n--- actual\n{}",
            path.display(),
            expected.unwrap_or_else(|error| format!("cannot read the file: {error}")),
            actual.join("\n")
        )
    })
}

fn golden(name: &str, build: impl Fn() -> Shell) {
    let failures: Vec<String> = SIZES
        .into_iter()
        .filter_map(|(width, height)| {
            let actual = lines(&render(build(), width, height));
            mismatch(name, width, height, &actual)
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn position(buffer: &Buffer, anchor: &str, needle: &str) -> (u16, u16) {
    for y in 2..buffer.area.height {
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        let found = row
            .find(anchor)
            .and_then(|at| row[at..].find(needle).map(|offset| at + offset));
        if let Some(offset) = found {
            return (u16::try_from(row[..offset].chars().count()).unwrap(), y);
        }
    }
    panic!(
        "{needle} after {anchor} not in\n{}",
        lines(buffer).join("\n")
    );
}

#[test]
fn the_chooser_frame_is_pinned() {
    golden("chooser", || model(true, false));
}

#[test]
fn the_services_frame_is_pinned() {
    golden("services", || model(false, false));
}

#[test]
fn the_following_logs_frame_is_pinned() {
    golden("logs-following", || logs_model(false));
}

#[test]
fn the_paused_logs_frame_is_pinned() {
    golden("logs-paused", || logs_model(true));
}

#[test]
fn the_sonda_frame_is_pinned() {
    golden("sonda", sonda_model);
}

#[test]
fn the_sonda_key_line_shows_only_with_the_focus_on_the_tabs() {
    let mut off_the_tabs = sonda_model();
    press(&mut off_the_tabs, KeyCode::Esc);

    let on_tabs = lines(&render(sonda_model(), 100, 30)).join("\n");
    let on_list = lines(&render(off_the_tabs, 100, 30)).join("\n");

    assert!(on_tabs.contains("q quit  j/k select"), "{on_tabs}");
    assert!(on_list.contains("SONDA"), "{on_list}");
    assert!(!on_list.contains("q quit  j/k select"), "{on_list}");
}

#[test]
fn the_help_frame_is_pinned() {
    golden("help", || pressed('?'));
}

#[test]
fn the_lab_help_frame_is_pinned() {
    golden("help-lab", || {
        let mut model = lab_model();
        press(&mut model, KeyCode::Char('?'));
        model
    });
}

#[test]
fn the_devices_help_frame_is_pinned() {
    golden("help-devices", || {
        let mut model = devices_model();
        press(&mut model, KeyCode::Char('?'));
        model
    });
}

#[test]
fn the_error_frame_is_pinned() {
    golden("error", || model(false, true));

    let mut last = model(false, true);
    press(&mut last, KeyCode::Tab);
    for _ in 0..6 {
        press(&mut last, KeyCode::Char('j'));
    }
    let screen = lines(&render(last, 60, 15));

    assert!(
        screen
            .iter()
            .any(|line| line.contains(">sonda") && line.contains("running")),
        "{}",
        screen.join("\n")
    );
}

#[test]
fn the_devices_frame_is_pinned() {
    golden("devices", devices_model);
}

#[test]
fn the_devices_interfaces_frame_is_pinned() {
    golden("devices-interfaces", || on_tab(1));
}

#[test]
fn the_devices_timeline_frame_is_pinned() {
    golden("devices-timeline", || on_tab(2));
}

#[test]
fn the_devices_popup_frame_is_pinned() {
    golden("devices-popup", || {
        let mut model = on_tab(2);
        press(&mut model, KeyCode::Enter);
        model
    });
}

#[test]
fn the_devices_logs_frame_is_pinned() {
    golden("devices-logs", || on_tab(3));
}

#[test]
fn the_devices_without_a_source_frame_is_pinned() {
    golden("devices-none", without_a_source);
}

#[test]
fn the_lab_frame_is_pinned() {
    golden("lab", || {
        let mut model = deployed(End::Exited(0));
        press(&mut model, KeyCode::Esc);
        model
    });
}

#[test]
fn the_run_pane_frame_is_pinned() {
    golden("run-pane", || pressed('o'));
}

#[test]
fn the_running_action_frames_are_pinned() {
    golden("run-running", || ran(None, false));
    golden("run-running-folded", || ran(None, true));
}

#[test]
fn the_finished_action_frames_are_pinned() {
    golden("run-exit-0", || ran(Some(End::Exited(0)), false));
    golden("run-exit-0-folded", || ran(Some(End::Exited(0)), true));
}

#[test]
fn the_failed_action_frames_are_pinned() {
    golden("run-exit-1", || ran(Some(End::Exited(1)), false));
    golden("run-exit-1-folded", || ran(Some(End::Exited(1)), true));
}

#[test]
fn an_action_that_did_not_stop_is_red_on_its_title_and_last_line_and_marks_its_row() {
    pinned(not_stopped(), "run-not-stopped", 100, 30);

    let buffer = render(not_stopped(), 100, 30);
    let (x, y) = position(&buffer, " run  autocon5 up", "not stopped");
    assert_eq!(buffer[(x, y)].fg, Color::Red);
    assert!(row_has_no_green(&buffer, y));
    let (x, y) = position(&buffer, "nobs: the action did not stop", "nobs:");
    assert_eq!(buffer[(x, y)].fg, Color::Red);
    assert!(lines(&buffer).iter().any(|line| line.contains("!autocon5")));
}

#[test]
fn a_failed_deploy_is_red_on_its_title_and_its_nobs_line_and_marks_its_chapter() {
    let buffer = render(refused(), 100, 30);

    let (x, y) = position(&buffer, " run  lab deploy -s ch1", "exit 1");
    assert_eq!(buffer[(x, y)].fg, Color::Red);
    assert!(row_has_no_green(&buffer, y));
    let (x, y) = position(&buffer, "nobs: lab deploy: node containers", "nobs:");
    assert_eq!(buffer[(x, y)].fg, Color::Red);
    assert!(lines(&buffer).iter().any(|line| line.contains("!ch1")));
}

#[test]
fn a_failed_maintenance_marks_its_device_row() {
    let source =
        Fixed::from_lab_vars(&lab_vars::read(&fs::read_to_string(WORKSHOP_ONE).unwrap()).unwrap());
    let devices = source.devices().unwrap();
    let dir = PathBuf::from(WORKSHOP_DIR);
    let lab = locate(&dir, Some(&dir)).unwrap();
    let mut model = model_with(false, false, Some(Arc::new(source)), Some(lab));
    press(&mut model, KeyCode::Char('1'));
    model.apply(Update::Devices(Ok(devices)));
    press(&mut model, KeyCode::Char('m'));
    press(&mut model, KeyCode::Char('y'));
    model.apply(Update::ActionEnded(End::Exited(1)));

    let screen = lines(&render(model, 100, 30));

    assert!(
        screen.iter().any(|line| line.starts_with("│!  srl1")),
        "{}",
        screen.join("\n")
    );
}

#[test]
fn nodes_show_question_marks_until_a_good_answer_whatever_the_error_row_does() {
    let left_of = |screen: &[String], name: &str| {
        let line = screen.iter().find(|line| line.contains(name)).unwrap();
        line.split("││").next().unwrap().to_string()
    };
    let unknown = |screen: &[String]| {
        let text = screen.join("\n");
        assert!(text.contains("│ nodes     ?"), "{text}");
        for name in ["nobs-t-srl1", "nobs-t-srl2"] {
            let row = left_of(screen, name);
            assert!(row.contains('?') && !row.contains("running"), "{row}");
        }
    };
    let waiting = lines(&render(unpolled_lab_model(LAB_DIR), 100, 30)).join("\n");
    assert!(waiting.contains("│ nodes     ?"), "{waiting}");

    let mut model = lab_model();
    model.apply(Update::Nodes(Err("docker is gone".to_string())));
    press(&mut model, KeyCode::Char('x'));
    let failed = lines(&render(model, 100, 30));
    unknown(&failed);
    assert!(!failed.join("\n").contains("docker is gone"));

    let mut model = lab_model();
    model.apply(Update::Nodes(Err("docker is gone".to_string())));
    model.apply(Update::Services(1, Err("exit 1".to_string())));
    unknown(&lines(&render(model, 100, 30)));

    let mut model = lab_model();
    model.apply(Update::Stopped("nodes".to_string()));
    press(&mut model, KeyCode::Char('x'));
    unknown(&lines(&render(model, 100, 30)));

    let mut model = lab_model();
    model.apply(Update::Nodes(Err("docker is gone".to_string())));
    model.apply(Update::Nodes(Ok(vec![node(
        "nobs-t-srl1",
        Some("running"),
    )])));
    let answered = lines(&render(model, 100, 30));
    assert!(left_of(&answered, "nobs-t-srl1").contains("running"));
    assert!(answered.join("\n").contains("│ nodes     1 of 1 running"));
}

#[test]
fn a_long_refusal_wraps_whole_into_the_run_pane_and_is_red_on_every_row() {
    golden("run-refused", refused);

    let buffer = render(refused(), 100, 30);

    for needle in [
        "nobs: lab deploy: node containers",
        "run nobs lab destroy first",
    ] {
        let (x, y) = position(&buffer, needle, needle);
        let start = (0..x)
            .rev()
            .find(|x| buffer[(*x, y)].symbol() == "│")
            .unwrap()
            + 1;
        assert!(
            (start..x + 5).all(|x| buffer[(x, y)].fg == Color::Red),
            "{needle}"
        );
    }
}

#[test]
fn a_short_run_pane_cuts_the_command_line_to_keep_the_whole_refusal() {
    pinned(refused_in(LONG_LAB_DIR), "run-refused-long", 60, 15);
}

#[test]
fn under_an_error_row_the_run_pane_grows_past_half_to_keep_the_whole_refusal() {
    let mut model = refused_in(LONG_LAB_DIR);
    model.apply(Update::Services(1, Err("exit 1".to_string())));

    pinned(model, "run-refused-error", 60, 15);
}

fn long_refusal() -> String {
    let names: Vec<String> = (1..=10)
        .map(|node| format!("clab-nobs-t-book-nobs-t-srl{node}"))
        .collect();
    format!(
        "nobs: lab deploy: node containers already exist ({}); run nobs lab destroy first",
        names.join(", ")
    )
}

#[test]
fn the_run_pane_stops_growing_where_the_selected_chapter_would_go_out_of_view() {
    let mut model = lab_model();
    press(&mut model, KeyCode::Char('j'));
    press(&mut model, KeyCode::Char('D'));
    press(&mut model, KeyCode::Char('y'));
    model.apply(Update::ActionLine(long_refusal()));
    model.apply(Update::ActionEnded(End::Exited(1)));
    model.apply(Update::Services(1, Err("exit 1".to_string())));

    pinned(model, "run-least-lab", 60, 15);
}

#[test]
fn on_the_devices_workbench_the_run_pane_leaves_seven_rows_to_the_devices() {
    let source =
        Fixed::from_lab_vars(&lab_vars::read(&fs::read_to_string(WORKSHOP_ONE).unwrap()).unwrap());
    let devices = source.devices().unwrap();
    let dir = PathBuf::from(WORKSHOP_DIR);
    let lab = locate(&dir, Some(&dir)).unwrap();
    let mut model = model_with(false, true, Some(Arc::new(source)), Some(lab));
    press(&mut model, KeyCode::Char('1'));
    model.apply(Update::Devices(Ok(devices)));
    press(&mut model, KeyCode::Char('m'));
    press(&mut model, KeyCode::Char('y'));
    model.apply(Update::ActionLine(long_refusal()));
    model.apply(Update::ActionEnded(End::Exited(1)));

    let screen = lines(&render(model, 60, 15));

    let top = screen
        .iter()
        .position(|line| line.starts_with("┌ run"))
        .unwrap();
    assert_eq!(screen.len() - 1 - top, 5, "{}", screen.join("\n"));
}

#[test]
fn a_long_question_wraps_whole_onto_the_bottom_rows() {
    golden("question-long", long_question);
}

#[test]
fn a_long_command_wraps_whole_onto_the_first_rows_of_the_run_pane() {
    golden("run-long", || {
        let mut model = long_question();
        press(&mut model, KeyCode::Char('y'));
        model.apply(Update::ActionLine(
            " Container nobs-t-packt-sonda-1  Starting".to_string(),
        ));
        model
    });
}

#[test]
fn the_question_frame_is_pinned() {
    let mut model = model(false, false);
    press(&mut model, KeyCode::Char('j'));
    press(&mut model, KeyCode::Char('u'));

    pinned(model, "question", 100, 30);
}

#[test]
fn three_and_the_lab_switcher_entry_exist_only_in_a_book_lab() {
    let workshop = pressed('3');
    let book = || {
        let mut model = lab_model();
        model.apply(Update::Devices(Ok(Fixed::from_lab_vars(
            &lab_vars::read(&fs::read_to_string(WORKSHOP_ONE).unwrap()).unwrap(),
        )
        .devices()
        .unwrap())));
        press(&mut model, KeyCode::Char('1'));
        model
    };

    assert_eq!(workshop.workbench(), Workbench::Services);
    let screen = lines(&render(workshop, 100, 30));
    assert!(!screen[1].contains("3 lab"), "{}", screen[1]);
    assert!(
        screen[1].starts_with(" 1 devices  [2 services]"),
        "{}",
        screen[1]
    );
    let screen = lines(&render(book(), 100, 30));
    assert!(screen[1].contains("3 lab"), "{}", screen[1]);
    assert!(
        screen[29].starts_with(" Tab focus  j/k move"),
        "{}",
        screen[29]
    );
    assert!(!screen[29].contains("m maintenance"), "{}", screen[29]);
}

#[test]
fn the_too_small_frame_is_pinned() {
    pinned(model(false, false), "too-small", 59, 15);
}

#[test]
fn paused_logs_scroll_back_by_the_offset() {
    let mut model = logs_model(true);
    press(&mut model, KeyCode::Char('k'));

    let screen = lines(&render(model, 100, 30)).join("\n");

    assert!(screen.contains("ring state ACTIVE"), "{screen}");
    assert!(!screen.contains("Loki started"), "{screen}");
}

#[test]
fn the_state_colours_are_fixed() {
    let buffer = render(model(false, false), 100, 30);
    let cell = |anchor: &str, needle: &str| {
        let (x, y) = position(&buffer, anchor, needle);
        buffer[(x, y)].clone()
    };

    assert_eq!(cell("grafana", "running").fg, Color::Green);
    assert_eq!(cell("loki", "down").fg, Color::Red);
    assert_eq!(cell("infrahub", "http 503").fg, Color::Yellow);
    assert!(cell("grafana", "41 seconds")
        .modifier
        .contains(Modifier::DIM));
    assert_eq!(cell("autocon5", "up 7/7").fg, Color::Green);
    assert!(cell("demo", "down 0/3").modifier.contains(Modifier::DIM));
}

#[test]
fn the_device_colours_are_fixed() {
    let buffer = render(devices_model(), 100, 30);
    let cell = |anchor: &str, needle: &str| {
        let (x, y) = position(&buffer, anchor, needle);
        buffer[(x, y)].clone()
    };

    assert_eq!(cell("srl1", "warn").fg, Color::Yellow);
    assert_eq!(cell("srl2", "ok").fg, Color::Green);
    assert_eq!(cell("M srl2", "M").fg, Color::Yellow);
    assert!(cell("srl2", "—").modifier.contains(Modifier::DIM));
    assert_eq!(cell("BgpSessionNotUp  srl1", "firing").fg, Color::Red);
    assert!(cell("BgpSessionNotUp  srl1", "silenced")
        .modifier
        .contains(Modifier::DIM));
    assert_eq!(cell("10.1.99.2", "active").fg, Color::Yellow);
    assert_eq!(cell("10.1.99.2", "! expected").fg, Color::Yellow);
    assert_eq!(cell("10.1.2.2", "up").fg, Color::Green);
}

#[test]
fn the_overview_draws_no_judgement_from_unknown_data() {
    let overview = |metrics: Result<Vec<Sample>, String>, alerts: Option<Vec<Alert>>| {
        let source = Fixed::from_lab_vars(
            &lab_vars::read(&fs::read_to_string(WORKSHOP_ONE).unwrap()).unwrap(),
        );
        let devices = source.devices().unwrap();
        let mut model = model_with(false, false, Some(Arc::new(source)), None);
        press(&mut model, KeyCode::Char('1'));
        model.apply(Update::Devices(Ok(devices)));
        model.apply(Update::Metrics(metrics));
        if let Some(alerts) = alerts {
            model.apply(Update::Alerts(Ok(alerts)));
        }
        lines(&render(model, 100, 30)).join("\n")
    };

    let failed = overview(Err("cannot reach http://x".to_string()), None);
    let empty = overview(Ok(Vec::new()), None);
    let answered = overview(Ok(metrics()), Some(Vec::new()));

    for screen in [&failed, &empty] {
        assert!(!screen.contains("! expected"), "{screen}");
        assert!(screen.contains("alerts   ?"), "{screen}");
    }
    assert!(answered.contains("! expected"), "{answered}");
    assert!(answered.contains("alerts   none"), "{answered}");
}

#[test]
fn the_timeline_counts_line_follows_the_window() {
    let timeline_after = |keys: &str| {
        let mut model = on_tab(2);
        for key in keys.chars() {
            press(&mut model, KeyCode::Char(key));
        }
        lines(&render(model, 100, 30))
    };

    let narrower = timeline_after("-");
    let wider = timeline_after("-++");

    assert!(
        narrower.iter().any(|row| row.contains("last 15m")),
        "{narrower:#?}"
    );
    assert!(
        wider.iter().any(|row| row.contains("last 1h")),
        "{wider:#?}"
    );
}

#[test]
fn draw_in_draws_inside_its_area_and_nowhere_else() {
    let at = Instant::now();
    let mut sonda = data::Model::new("http://127.0.0.1:18122".to_string());
    sonda.apply(data::Update {
        at,
        list: Ok(SCENARIOS.to_string()),
        selected: None,
    });
    let area = Rect::new(10, 5, 40, 10);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    terminal
        .draw(|frame| view::draw_in(frame, area, &sonda, at))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let inside: String = (area.y..area.bottom())
        .flat_map(|y| (area.x..area.right()).map(move |x| (x, y)))
        .map(|(x, y)| buffer[(x, y)].symbol())
        .collect();
    assert!(inside.contains("ID"), "{inside}");
    let stray: Vec<(u16, u16)> = (0..24)
        .flat_map(|y| (0..80).map(move |x| (x, y)))
        .filter(|(x, y)| !area.contains(Position::new(*x, *y)) && buffer[(*x, *y)].symbol() != " ")
        .collect();
    assert!(stray.is_empty(), "{stray:?}");
}
