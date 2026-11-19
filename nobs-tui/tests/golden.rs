use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nobs_core::alertmanager::Alert;
use nobs_core::device::{PASSWORD, USER};
use nobs_core::lab::{services, Service};
use nobs_core::lab_vars;
use nobs_core::locate::{locate, Kind, Located};
use nobs_core::loki::Line as LokiLine;
use nobs_core::prefect::FlowRun;
use nobs_core::prometheus::Sample;
use nobs_core::sot::{Fixed, SourceOfTruth};
use nobs_core::status::{self, Urls};
use nobs_tui::actions::{Action, Row, ACTIONS};
use nobs_tui::book::NodeRow;
use nobs_tui::shell::{stacks, Health, Reach};
use nobs_tui::{
    data, draw, handle, settle, view, End, Focus, Shell, SondaTarget, Stack, StackKind, Start,
    Update, Workbench,
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
const PROJECT_LAB: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/fixtures/project-two-node"
);
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
const PACKT: [&str; 2] = ["grafana", "prometheus"];
const CH2: [&str; 1] = ["builder"];
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
        shows_sonda: false,
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

fn defined(names: &[&str]) -> Result<Vec<String>, String> {
    Ok(names.iter().map(|name| name.to_string()).collect())
}

fn listing(index: usize, rows: Vec<Service>) -> Update {
    let mut names: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    names.sort();
    names.dedup();
    Update::Services(index, Ok(names), Ok(rows))
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
        shows_sonda: true,
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
        source_service: None,
        urls: Urls::defaults(),
        program: PathBuf::from("nobs"),
        login: Arc::new(found),
    }
}

fn found(name: &str) -> Option<String> {
    match name {
        USER => Some("nobs-t-x".to_string()),
        PASSWORD => Some("nobs-t-not-a-secret".to_string()),
        _ => None,
    }
}

fn login_prompt(password: bool) -> Shell {
    let mut model = devices_model_finding(BOOK_LAB, "containerlab/lab_vars.yml", LAB_DIR, |_| None);
    press(&mut model, KeyCode::Char('g'));
    for typed in "nobs-t-x".chars() {
        press(&mut model, KeyCode::Char(typed));
    }
    if password {
        press(&mut model, KeyCode::Enter);
        for typed in "nobs-t-not-a-secret".chars() {
            press(&mut model, KeyCode::Char(typed));
        }
    }
    model
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
        stack(
            "autocon5",
            &[
                ("INFRAHUB_ADDRESS", "http://localhost:8000"),
                ("SONDA_SERVER_URL", "http://localhost:8085"),
            ],
        ),
        stack("packt", &[]),
        stack("demo", &[]),
    ];
    if chooser {
        stacks.push(sonda_only());
    }
    let mut model = Shell::new(Start {
        source,
        urls: Urls {
            set: vec![status::Service::Flows],
            ..Urls::defaults()
        },
        ..begin(lab, stacks, Workbench::Services)
    });
    if !chooser {
        press(&mut model, KeyCode::Enter);
    }
    model.tick(start, clock(), (80, 24));
    if packt_failed {
        model.apply(Update::Services(
            1,
            defined(&PACKT),
            Err("exit 1".to_string()),
        ));
        model.tick(start + Duration::from_secs(10), clock(), (80, 24));
    }
    model.apply(listing(0, ps(&WORKSHOP)));
    model.apply(Update::Health(0, health()));
    if !packt_failed {
        model.apply(Update::Services(1, defined(&PACKT), Ok(Vec::new())));
    }
    model.apply(listing(2, ps(&DEMO)));
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
        source: Some(Arc::new(lab_vars_source(
            &dir.join("containerlab/lab_vars.yml"),
        ))),
        ..begin(Some(lab), chapters, Workbench::Lab)
    });
    model.tick(Instant::now(), clock(), (80, 24));
    model.apply(listing(
        0,
        ps(&[("grafana", "running", 3000, "41 seconds ago")]),
    ));
    model.apply(Update::Services(1, defined(&CH2), Ok(Vec::new())));
    model
}

fn lab_vars_source(path: &Path) -> Fixed {
    let text = fs::read_to_string(path).unwrap();
    Fixed::from_lab_vars(&lab_vars::read(&text).unwrap())
}

fn lab_devices_model() -> Shell {
    devices_model_in(BOOK_LAB, "containerlab/lab_vars.yml", LAB_DIR)
}

fn devices_model_in(fixture: &str, lab_vars: &str, lab_dir: &str) -> Shell {
    devices_model_finding(fixture, lab_vars, lab_dir, found)
}

fn devices_model_finding(
    fixture: &str,
    lab_vars: &str,
    lab_dir: &str,
    login: fn(&str) -> Option<String>,
) -> Shell {
    let dir = PathBuf::from(fixture);
    let mut lab = locate(&dir, Some(&dir)).unwrap();
    let mut chapters = stacks(Some(&lab), Vec::new()).unwrap();
    for chapter in &mut chapters {
        chapter.path = PathBuf::from(lab_dir);
    }
    lab.dir = PathBuf::from(lab_dir);
    let source = lab_vars_source(&dir.join(lab_vars));
    let devices = source.devices().unwrap();
    let mut model = Shell::new(Start {
        chooser: false,
        source: Some(Arc::new(source)),
        login: Arc::new(login),
        ..begin(Some(lab), chapters, Workbench::Devices)
    });
    model.tick(Instant::now(), clock(), (80, 24));
    for index in 0..model.stacks().len() {
        model.apply(Update::Services(index, defined(&[]), Ok(Vec::new())));
    }
    model.apply(Update::Devices(Ok(devices)));
    model
}

fn on_lab_tab(presses: usize) -> Shell {
    let mut model = lab_devices_model();
    press(&mut model, KeyCode::Enter);
    for _ in 0..presses {
        press(&mut model, KeyCode::Char(']'));
    }
    model
}

fn config_shown() -> Shell {
    let mut model = on_lab_tab(0);
    press(&mut model, KeyCode::Char('g'));
    let first = "set / acl acl-filter cpm type ipv4 statistics-per-entry true".to_string();
    let entries = (1..=40).map(|entry| {
        format!(
            "set / acl acl-filter cpm type ipv4 entry {} match ipv4 protocol icmp",
            entry * 10
        )
    });
    for line in [first].into_iter().chain(entries) {
        model.apply(Update::ActionLine(line));
    }
    model.apply(Update::ActionEnded(End::Exited(0)));
    press(&mut model, KeyCode::Esc);
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
    deployed_after(41, end)
}

fn deployed_after(seconds: u64, end: End) -> Shell {
    let mut model = lab_model();
    press(&mut model, KeyCode::Char('D'));
    press(&mut model, KeyCode::Char('y'));
    later(&mut model, seconds);
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

fn answered(mut model: Shell, count: usize) -> Shell {
    for index in 1..=count {
        model.apply(Update::ActionLine(format!(" Step {index:02}  done")));
    }
    later(&mut model, 41);
    model.apply(Update::ActionEnded(End::Exited(0)));
    model
}

fn settled(model: &mut Shell, width: u16, height: u16) {
    model.tick(model.now(), clock(), (width, height));
    settle(model, Rect::new(0, 0, width, height));
}

fn focused(mut model: Shell, width: u16, height: u16) -> Shell {
    settled(&mut model, width, height);
    for _ in 0..4 {
        if model.focus() == Focus::Run {
            break;
        }
        press(&mut model, KeyCode::Tab);
        settled(&mut model, width, height);
    }
    model
}

fn scrolled(mut model: Shell, presses: usize, width: u16, height: u16) -> Shell {
    for _ in 0..presses {
        press(&mut model, KeyCode::Char('k'));
        settled(&mut model, width, height);
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
    golden_at(name, |_, _| build());
}

fn golden_at(name: &str, build: impl Fn(u16, u16) -> Shell) {
    let failures: Vec<String> = SIZES
        .into_iter()
        .filter_map(|(width, height)| {
            let actual = lines(&render(build(width, height), width, height));
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
fn the_logs_with_hidden_lines_frame_is_pinned() {
    golden("logs-hidden", || {
        let mut model = logs_model(false);
        let lines = (0..36).map(|index| format!("13:42:03 level=info msg=\"line {index:02}\""));
        model.apply(Update::Logs(0, "loki".to_string(), Ok(lines.collect())));
        press(&mut model, KeyCode::Char('f'));
        for _ in 0..3 {
            press(&mut model, KeyCode::Char('k'));
        }
        model
    });
}

#[test]
fn the_sonda_frame_is_pinned() {
    golden("sonda", sonda_model);
}

#[test]
fn the_sonda_details_with_the_focus_frame_is_pinned() {
    golden("sonda-lines", || {
        let mut model = sonda_model();
        press(&mut model, KeyCode::Tab);
        model
    });
}

#[test]
fn the_sonda_top_line_ends_at_a_whole_word_and_its_key_row_at_a_whole_hint_at_60_columns() {
    let rows = lines(&render(sonda_model(), 60, 15));

    assert!(
        rows[4].ends_with("││http://localhost:8085              │"),
        "{}",
        rows[4]
    );
    assert!(
        rows[11].ends_with("││q quit  j/k select  r refresh      │"),
        "{}",
        rows[11]
    );
}

fn sonda_failed(error: &str) -> Shell {
    let mut model = sonda_model();
    let at = model.now();
    model.apply(Update::Sonda(data::Update {
        at,
        list: Err(error.to_string()),
        selected: None,
    }));
    model
}

#[test]
fn inside_a_stack_at_60_columns_a_sonda_error_stands_alone_with_its_end_and_its_age() {
    let model = sonda_failed(
        "cannot reach http://localhost:8085/scenarios: io: Connection refused (os error 61)",
    );

    let rows = lines(&render(model, 60, 15));

    assert!(
        rows[4].ends_with("││… refused (os error 61) (2s ago)   │"),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn a_wide_sonda_top_line_keeps_the_server_the_age_and_a_short_error() {
    let rows = lines(&render(sonda_failed("refused"), 100, 30));

    assert!(
        rows.iter()
            .any(|row| row.contains("││http://localhost:8085  refreshed 2s ago  refused (2s ago)")),
        "{}",
        rows.join("\n")
    );
}

fn sonda_table_at_60(names: &[&str]) -> Vec<String> {
    let mut model = sonda_model();
    let at = model.now();
    let scenarios: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            format!(
                r#"{{"id":"a{index}","name":"{name}","state":"running","elapsed_secs":12,"degraded":false}}"#
            )
        })
        .collect();
    model.apply(Update::Sonda(data::Update {
        at,
        list: Ok(format!(r#"{{"scenarios":[{}]}}"#, scenarios.join(","))),
        selected: None,
    }));
    let rows = lines(&render(model, 60, 15));
    rows[6..7 + names.len()]
        .iter()
        .map(|row| row.split('║').nth(1).unwrap_or_default().to_string())
        .collect()
}

#[test]
fn inside_a_stack_at_60_columns_the_sonda_table_lets_state_go_before_it_cuts_a_name() {
    let short = sonda_table_at_60(&["cpu", "mem"]);
    assert!(
        short[0].contains("NAME") && short[0].contains("STATE"),
        "{short:?}"
    );
    assert!(
        short[1].contains("cpu") && short[1].contains("running"),
        "{short:?}"
    );

    let middle = sonda_table_at_60(&["interface-flap", "cpu"]);
    assert!(
        middle[0].contains("NAME") && !middle[0].contains("STATE"),
        "{middle:?}"
    );
    assert!(
        middle[1].contains("interface-flap") && !middle[1].contains("running"),
        "{middle:?}"
    );

    let long = sonda_table_at_60(&["a-scenario-name-longer-than-the-table"]);
    assert_eq!(long[1].trim_end(), "> a-scenario-nam…", "{long:?}");
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
fn the_devices_logs_with_hidden_lines_frame_is_pinned() {
    golden("devices-logs-hidden", || {
        let mut model = on_tab(3);
        let older = (1..=35)
            .map(|index| loki_line(100 + index, 0, &format!("log line {index:02}")))
            .collect();
        model.apply(Update::DeviceLogs("srl1".to_string(), Ok(older)));
        model
    });
}

#[test]
fn the_config_hint_frame_is_pinned() {
    golden("devices-config-hint", || on_lab_tab(2));
}

#[test]
fn the_config_frame_is_pinned() {
    golden("devices-config", config_shown);
}

#[test]
fn the_config_filter_frame_is_pinned() {
    golden("devices-config-filter", || {
        let mut model = config_shown();
        press(&mut model, KeyCode::Char('/'));
        for typed in "entry 1".chars() {
            press(&mut model, KeyCode::Char(typed));
        }
        model
    });
}

#[test]
fn the_entry_frame_is_pinned() {
    golden("devices-entry", || {
        let mut model = lab_devices_model();
        press(&mut model, KeyCode::Char('s'));
        for typed in "show version".chars() {
            press(&mut model, KeyCode::Char(typed));
        }
        model
    });
}

#[test]
fn the_login_frames_are_pinned() {
    golden("devices-login-user", || login_prompt(false));
    golden("devices-login-password", || login_prompt(true));
}

#[test]
fn the_lab_logs_tab_line_frame_is_pinned() {
    pinned(on_lab_tab(4), "devices-lab-logs", 60, 15);
}

#[test]
fn the_project_devices_frame_is_pinned() {
    let model = devices_model_in(
        PROJECT_LAB,
        "expected/lab_vars.yml",
        "/home/me/nobs-t-project",
    );
    pinned(model, "devices-project", 100, 30);
}

#[test]
fn the_lab_devices_help_frame_is_pinned() {
    golden("help-devices-lab", || {
        let mut model = lab_devices_model();
        press(&mut model, KeyCode::Char('?'));
        model
    });
}

#[test]
fn the_devices_without_a_source_frame_is_pinned() {
    golden("devices-none", without_a_source);
}

fn workshop_down() -> Shell {
    let mut model = workshop_unpolled();
    let names: Vec<&str> = status::SERVICES.iter().map(|row| row.compose).collect();
    model.apply(Update::Services(0, defined(&names), Ok(Vec::new())));
    model
}

fn workshop_unpolled() -> Shell {
    let dir = PathBuf::from(WORKSHOP_DIR);
    let mut lab = locate(&dir, Some(&dir)).unwrap();
    lab.dir = PathBuf::from("/home/me/ws/autocon5");
    if let Kind::Workshop(workshop) = &mut lab.kind {
        workshop.name = "autocon5".to_string();
        workshop.sot = "infrahub".to_string();
    }
    let source = lab_vars_source(Path::new(WORKSHOP_ONE));
    let mut model = Shell::new(Start {
        chooser: false,
        source: Some(Arc::new(source)),
        source_service: Some(status::Service::Sot),
        ..begin(Some(lab), vec![stack("autocon5", &[])], Workbench::Devices)
    });
    model.tick(Instant::now(), clock(), (80, 24));
    model
}

#[test]
fn the_workshop_with_its_stack_down_frame_is_pinned() {
    golden("devices-workshop-down", workshop_down);
}

fn without_a_compose_file() -> Shell {
    let dir = PathBuf::from(PROJECT_LAB);
    let mut lab = locate(&dir, Some(&dir)).unwrap();
    let mut chapters = stacks(Some(&lab), Vec::new()).unwrap();
    for chapter in &mut chapters {
        chapter.path = PathBuf::from("/home/me/nobs-t-project");
    }
    lab.dir = PathBuf::from("/home/me/nobs-t-project");
    let mut model = Shell::new(Start {
        chooser: false,
        ..begin(Some(lab), chapters, Workbench::Services)
    });
    model.tick(Instant::now(), clock(), (80, 24));
    model.apply(Update::Services(0, defined(&[]), Ok(Vec::new())));
    model
}

#[test]
fn the_services_frame_of_a_lab_without_a_compose_file_is_pinned() {
    pinned(without_a_compose_file(), "services-no-compose", 100, 30);
}

#[test]
fn a_lab_without_a_compose_file_says_it_has_no_services_and_shows_no_ps_red_line() {
    let screen = lines(&render(without_a_compose_file(), 100, 30));

    assert!(
        screen
            .iter()
            .any(|line| line.contains("no services in this lab")),
        "{}",
        screen.join("\n")
    );
    assert!(
        !screen.iter().any(|line| line.starts_with(" ps ")),
        "{}",
        screen.join("\n")
    );
}

const STATES: [Reach; 5] = [
    Reach::Unknown,
    Reach::NotInLab,
    Reach::StackDown,
    Reach::Answers,
    Reach::NoAnswer,
];

fn shown_rows() -> impl Iterator<Item = &'static status::Row> {
    status::SERVICES
        .iter()
        .filter(|row| row.service != status::Service::Grafana)
}

fn answers_of(
    service: status::Service,
    devices: Vec<nobs_core::sot::Device>,
    at: Instant,
) -> Vec<Update> {
    let srl1 = || "srl1".to_string();
    match service {
        status::Service::Metrics => vec![Update::Metrics(Ok(metrics()))],
        status::Service::Alerts => vec![Update::Alerts(Ok(alerts()))],
        status::Service::Logs => vec![
            Update::DeviceLogs(srl1(), Ok(device_logs())),
            Update::Rcas(srl1(), Ok(rcas())),
        ],
        status::Service::Flows => vec![Update::FlowRuns(Ok(flow_runs()))],
        status::Service::Sot => vec![Update::Devices(Ok(devices))],
        status::Service::Sonda => vec![Update::Sonda(data::Update {
            at,
            list: Ok(SCENARIOS.to_string()),
            selected: None,
        })],
        status::Service::Grafana => Vec::new(),
    }
}

fn failure_of(service: status::Service, at: Instant) -> Update {
    let text = "cannot reach http://localhost:1: connection refused".to_string();
    match service {
        status::Service::Metrics => Update::Metrics(Err(text)),
        status::Service::Alerts => Update::Alerts(Err(text)),
        status::Service::Logs => Update::DeviceLogs("srl1".to_string(), Err(text)),
        status::Service::Flows => Update::FlowRuns(Err(text)),
        status::Service::Sot => Update::Devices(Err(text)),
        status::Service::Sonda | status::Service::Grafana => Update::Sonda(data::Update {
            at,
            list: Err(text),
            selected: None,
        }),
    }
}

fn lab_where(row: &status::Row, reach: Reach) -> Shell {
    let source = lab_vars_source(Path::new(WORKSHOP_ONE));
    let devices = source.devices().unwrap();
    let sot = row.service == status::Service::Sot;
    let mut model = Shell::new(Start {
        chooser: false,
        source: Some(Arc::new(source)),
        source_service: sot.then_some(row.service),
        ..begin(None, vec![stack("ws", &[])], Workbench::Services)
    });
    model.tick(Instant::now(), clock(), (80, 24));
    match reach {
        Reach::Unknown => {}
        Reach::NotInLab => {
            model.apply(Update::Services(0, defined(&[]), Ok(Vec::new())));
        }
        Reach::StackDown => {
            model.apply(Update::Services(0, defined(&[row.compose]), Ok(Vec::new())));
        }
        Reach::Answers | Reach::NoAnswer => {
            model.apply(listing(
                0,
                ps(&[(row.compose, "running", 1, "1 second ago")]),
            ));
        }
    }
    model.apply(Update::Health(
        0,
        vec![Health {
            service: row.compose.to_string(),
            state: "ok".to_string(),
        }],
    ));
    if !sot {
        model.apply(Update::Devices(Ok(devices.clone())));
    }
    let at = model.now();
    if reach == Reach::Answers || (reach == Reach::NoAnswer && !sot) {
        for update in answers_of(row.service, devices, at) {
            model.apply(update);
        }
    }
    if reach != Reach::Answers {
        model.apply(failure_of(row.service, at));
    }
    model
}

fn places(row: &status::Row, reach: Reach) -> Vec<String> {
    let mut model = lab_where(row, reach);
    press(&mut model, KeyCode::Char('1'));
    let mut screens = vec![lines(&render(model, 100, 30)).join("\n")];
    for presses in 0..5 {
        let mut model = lab_where(row, reach);
        press(&mut model, KeyCode::Char('1'));
        press(&mut model, KeyCode::Enter);
        for _ in 0..presses {
            press(&mut model, KeyCode::Char(']'));
        }
        screens.push(lines(&render(model, 100, 30)).join("\n"));
    }
    for presses in 0..3 {
        let mut model = lab_where(row, reach);
        press(&mut model, KeyCode::Tab);
        for _ in 0..presses {
            press(&mut model, KeyCode::Char(']'));
        }
        screens.push(lines(&render(model, 100, 30)).join("\n"));
    }
    screens
}

fn panes_of(line: &str) -> Vec<&str> {
    line.split(['│', '║']).collect()
}

fn cells_after<'a>(screen: &'a str, pane: usize, first: &str) -> Vec<&'a str> {
    screen
        .lines()
        .filter_map(|line| panes_of(line).get(pane).copied())
        .map(|text| text.split_whitespace().collect::<Vec<_>>())
        .find(|cells| {
            cells
                .iter()
                .any(|cell| cell.trim_start_matches('>') == first)
        })
        .unwrap_or_default()
}

fn value(reach: Reach, answered: &'static str) -> &'static str {
    match reach {
        Reach::Answers => answered,
        Reach::NoAnswer => "?",
        Reach::Unknown | Reach::NotInLab | Reach::StackDown => "—",
    }
}

#[test]
fn one_state_drives_every_place_that_shows_the_data_of_a_service() {
    for row in shown_rows() {
        for reach in STATES {
            let screens = places(row, reach);
            let all = screens.join("\n");
            let label = format!("{} {reach:?}\n{all}", row.word);
            let no_word = format!("no {} in this lab", row.word);
            match reach {
                Reach::NotInLab => assert!(all.contains(&no_word), "{label}"),
                Reach::StackDown => assert!(all.contains("the stack is down"), "{label}"),
                Reach::Unknown | Reach::Answers | Reach::NoAnswer => {
                    assert!(!all.contains(&no_word), "{label}");
                    assert!(!all.contains("the stack is down"), "{label}");
                }
            }
            let red = format!(" {}: ", row.word);
            let red_line = all.lines().any(|line| line.starts_with(&red));
            let failed = reach == Reach::NoAnswer && row.service != status::Service::Sonda;
            assert_eq!(red_line, failed, "{label}");
            let services = screens[screens.len() - 3..]
                .iter()
                .find(|screen| screen.contains(" SERVICE "))
                .unwrap();
            let checked = match reach {
                Reach::Answers | Reach::NoAnswer => Some("ok"),
                Reach::StackDown => Some("—"),
                Reach::Unknown | Reach::NotInLab => None,
            };
            let health_cell = cells_after(services, 3, row.compose).get(2).copied();
            assert_eq!(health_cell, checked, "{} {reach:?}\n{services}", row.word);
            let answered = reach == Reach::Answers;
            let list = &screens[0];
            match row.service {
                status::Service::Metrics => {
                    let health = cells_after(list, 1, "srl1").get(3).copied();
                    assert_eq!(health, Some(value(reach, "warn")), "{label}");
                    let bgp = cells_after(list, 3, "10.1.2.2").get(2).copied();
                    assert_eq!(bgp, Some(value(reach, "up")), "{label}");
                    let interface = cells_after(&all, 3, "ethernet-1/1");
                    let states = (interface.get(2).copied(), interface.get(5).copied());
                    let up = Some(value(reach, "up"));
                    assert_eq!(states, (up, up), "{label}");
                }
                status::Service::Alerts => {
                    let cells = cells_after(list, 1, "srl1");
                    assert_eq!(cells.get(5).copied(), Some(value(reach, "1")), "{label}");
                    let title = format!(" alerts {} ", value(reach, "2"));
                    assert!(list.contains(&title), "{label}");
                    let line = format!("alerts   {}", value(reach, "BgpSessionNotUp"));
                    assert!(list.contains(&line), "{label}");
                    assert_eq!(list.contains("BgpSessionNotUp  srl1"), answered, "{label}");
                    assert_eq!(all.contains("alert  BgpSessionNotUp"), answered, "{label}");
                }
                status::Service::Logs => {
                    assert_eq!(all.contains("state changed to idle"), answered, "{label}");
                    assert_eq!(all.contains("AI RCA"), answered, "{label}");
                }
                status::Service::Flows => {
                    assert_eq!(all.contains("evidence | srl1"), answered, "{label}");
                }
                status::Service::Sot => {
                    assert_eq!(
                        !cells_after(list, 1, "srl1").is_empty(),
                        answered,
                        "{label}"
                    );
                    let waiting = matches!(reach, Reach::Unknown | Reach::NoAnswer);
                    assert_eq!(list.contains("no answer yet"), waiting, "{label}");
                }
                status::Service::Sonda => {
                    let sonda = &screens[screens.len() - 3..].join("\n");
                    let kept = matches!(reach, Reach::Answers | Reach::NoAnswer);
                    assert_eq!(sonda.contains("running 12"), kept, "{label}");
                    let refused = sonda.contains("connection refused");
                    assert_eq!(refused, reach == Reach::NoAnswer, "{label}");
                }
                status::Service::Grafana => {}
            }
        }
    }
}

const TOOLS: [&str; 4] = ["prometheus", "loki", "alertmanager", "prefect"];

fn tools_not_named_as_real_things(line: &str) -> Vec<&'static str> {
    let lower = line.to_lowercase();
    TOOLS
        .iter()
        .flat_map(|tool| lower.match_indices(tool).map(|(at, _)| (at, *tool)))
        .filter(|(at, tool)| {
            let (before, after) = (&lower[..*at], &lower[at + tool.len()..]);
            let after = after.trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '-');
            let service_row = before.trim_end_matches([' ', '>']).ends_with(['│', '║'])
                && after.starts_with("  ");
            let followed = after.starts_with("  following") || after.starts_with("  paused");
            let log_line = before.contains("msg=\"");
            !(service_row || followed || log_line)
        })
        .map(|(_, tool)| tool)
        .collect()
}

#[test]
fn no_frame_and_no_red_line_names_a_tool_except_as_the_name_of_a_real_thing() {
    let mut found = Vec::new();
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    for entry in fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        for line in fs::read_to_string(&path).unwrap().lines() {
            for tool in tools_not_named_as_real_things(line) {
                found.push(format!("{} {tool}: {line}", path.display()));
            }
        }
    }
    for row in shown_rows() {
        for reach in STATES {
            for screen in places(row, reach) {
                for line in screen.lines() {
                    for tool in tools_not_named_as_real_things(line) {
                        found.push(format!("{} {reach:?} {tool}: {line}", row.word));
                    }
                }
            }
        }
    }

    assert!(found.is_empty(), "{}", found.join("\n"));
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
fn the_lab_overview_with_the_focus_frame_is_pinned() {
    golden("lab-lines", || {
        let mut model = lab_model();
        press(&mut model, KeyCode::Tab);
        press(&mut model, KeyCode::Tab);
        model
    });
}

#[test]
fn a_narrow_run_title_cuts_its_command_to_keep_its_time_whole_and_a_gap_before_the_hint() {
    let mut folded = deployed_after(120, End::Exited(0));
    press(&mut folded, KeyCode::Esc);

    pinned(deployed_after(120, End::Exited(0)), "run-long-time", 60, 15);
    pinned(folded, "run-long-time-folded", 60, 15);
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
fn the_run_pane_with_the_focus_frames_are_pinned() {
    let over = |width, height| focused(answered(ran(None, false), 38), width, height);
    golden_at("run-focus-short", |width, height| {
        let up = [KeyCode::Char('u'), KeyCode::Char('y')];
        focused(answered(after(model(false, false), &up), 1), width, height)
    });
    golden_at("run-focus-fits", |width, height| {
        focused(answered(ran(None, false), 12), width, height)
    });
    golden_at("run-focus-over", over);
    golden_at("run-focus-moved", |width, height| {
        scrolled(over(width, height), 3, width, height)
    });
    golden_at("run-focus-top", |width, height| {
        scrolled(over(width, height), 50, width, height)
    });
}

#[test]
fn other_workbenches_draw_above_a_run_pane_with_the_focus() {
    use KeyCode::{Char, Enter};
    let lab = answered(after(lab_model(), &[Char('D'), Char('y')]), 9);
    let typed: Vec<KeyCode> = [Char('s')]
        .into_iter()
        .chain("show version".chars().map(Char))
        .chain([Enter])
        .collect();
    let devices = answered(after(lab_devices_model(), &typed), 6);

    pinned(focused(lab, 100, 30), "run-focus-lab", 100, 30);
    pinned(focused(devices, 60, 15), "run-focus-devices", 60, 15);
}

#[test]
fn the_help_frame_with_the_run_pane_drawn_is_pinned() {
    golden_at("help-run", |width, height| {
        let mut model = ran(Some(End::Exited(0)), false);
        settled(&mut model, width, height);
        press(&mut model, KeyCode::Char('?'));
        model
    });
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

    let (x, y) = position(
        &buffer,
        " run  lab deploy --sudo-no-prompt -s ch1",
        "exit 1",
    );
    assert_eq!(buffer[(x, y)].fg, Color::Red);
    assert!(row_has_no_green(&buffer, y));
    let (x, y) = position(&buffer, "nobs: lab deploy: node containers", "nobs:");
    assert_eq!(buffer[(x, y)].fg, Color::Red);
    assert!(lines(&buffer).iter().any(|line| line.contains("!ch1")));
}

#[test]
fn the_question_and_the_run_pane_show_each_lab_command_with_the_sudo_flag() {
    for (key, word, clause) in [
        (
            'D',
            "deploy",
            " It takes the other chapters' services down.",
        ),
        (
            'X',
            "destroy",
            " It removes the chapter's volumes, every node and the other chapters' services.",
        ),
    ] {
        let command = format!("nobs --dir {LAB_DIR} lab {word} --sudo-no-prompt -s ch1");
        let asking = || {
            let mut model = lab_model();
            press(&mut model, KeyCode::Char(key));
            model
        };
        let mut running = asking();
        press(&mut running, KeyCode::Char('y'));

        let asked = lines(&render(asking(), 100, 30));
        let start = asked
            .iter()
            .position(|line| line.starts_with(" run nobs"))
            .unwrap();
        let rows: Vec<&str> = asked[start..].iter().map(|line| line.trim()).collect();
        let question = rows.join(" ");
        let pane = lines(&render(running, 100, 30)).join("\n");

        assert!(
            question.contains(&format!("run {command}?{clause} y/n")),
            "{question}"
        );
        assert!(pane.contains(&format!("$ {command}")), "{pane}");
    }
}

fn maintenance_model(packt_failed: bool) -> Shell {
    let source =
        Fixed::from_lab_vars(&lab_vars::read(&fs::read_to_string(WORKSHOP_ONE).unwrap()).unwrap());
    let devices = source.devices().unwrap();
    let dir = PathBuf::from(WORKSHOP_DIR);
    let lab = locate(&dir, Some(&dir)).unwrap();
    let mut model = model_with(false, packt_failed, Some(Arc::new(source)), Some(lab));
    press(&mut model, KeyCode::Char('1'));
    model.apply(Update::Devices(Ok(devices)));
    model
}

fn maintained(packt_failed: bool) -> Shell {
    let mut model = maintenance_model(packt_failed);
    press(&mut model, KeyCode::Char('m'));
    press(&mut model, KeyCode::Char('y'));
    model
}

#[test]
fn a_failed_maintenance_marks_its_device_row() {
    let mut model = maintained(false);
    model.apply(Update::ActionEnded(End::Exited(1)));

    let screen = lines(&render(model, 100, 30));

    assert!(
        screen.iter().any(|line| line.starts_with("║!  srl1")),
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
    model.apply(Update::Services(
        1,
        defined(&CH2),
        Err("exit 1".to_string()),
    ));
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
fn a_long_node_name_never_cuts_the_state_title() {
    let long = "nobs-t-a-node-name-longer-than-any-pane-of-the-lab-workbench";
    for (width, height) in SIZES {
        let mut model = lab_model();
        model.apply(Update::Nodes(Ok(vec![node(long, Some("running"))])));

        let screen = lines(&render(model, width, height));

        let top = screen
            .iter()
            .position(|line| line.contains("┌ nodes"))
            .unwrap();
        let pane = |row: usize| screen[row].split("││").next().unwrap().to_string();
        assert!(pane(top + 1).contains("  STATE"), "{}", screen.join("\n"));
        let row = pane(top + 2);
        assert!(
            row.contains("…  ") && row.contains("  running"),
            "{}",
            screen.join("\n")
        );
    }
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
fn a_short_run_pane_drops_the_command_line_to_keep_the_whole_refusal() {
    pinned(refused_in(LONG_LAB_DIR), "run-refused-long", 60, 15);
}

#[test]
fn under_an_error_row_the_run_pane_gives_way_to_the_lab_minimum_and_keeps_the_newest_rows() {
    let mut model = refused_in(LONG_LAB_DIR);
    model.apply(Update::Services(
        1,
        defined(&CH2),
        Err("exit 1".to_string()),
    ));

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

fn with_long_refusal(mut model: Shell) -> Shell {
    model.apply(Update::ActionLine(long_refusal()));
    model.apply(Update::ActionEnded(End::Exited(1)));
    model
}

fn chapter_two_refused() -> Shell {
    let mut model = lab_model();
    press(&mut model, KeyCode::Char('j'));
    press(&mut model, KeyCode::Char('D'));
    press(&mut model, KeyCode::Char('y'));
    let mut model = with_long_refusal(model);
    model.apply(Update::Services(
        1,
        defined(&CH2),
        Err("exit 1".to_string()),
    ));
    model
}

fn second_stack_refused() -> Shell {
    let mut model = model(false, true);
    press(&mut model, KeyCode::Char('j'));
    press(&mut model, KeyCode::Char('u'));
    press(&mut model, KeyCode::Char('y'));
    with_long_refusal(model)
}

fn pane_rows(model: Shell, width: u16, height: u16) -> usize {
    let screen = lines(&render(model, width, height));
    let top = screen
        .iter()
        .position(|line| line.starts_with("┌ run"))
        .unwrap();
    screen.len() - 1 - top
}

#[test]
fn the_run_pane_stops_at_the_lab_minimum_and_the_chapters_scroll_to_the_selected_one() {
    pinned(chapter_two_refused(), "run-least-lab", 60, 15);
}

#[test]
fn the_run_pane_grows_past_half_only_up_to_each_workbench_minimum() {
    let heights = vec![
        ("lab", pane_rows(chapter_two_refused(), 60, 15)),
        ("services", pane_rows(second_stack_refused(), 60, 15)),
        (
            "devices",
            pane_rows(with_long_refusal(maintained(true)), 60, 20),
        ),
    ];

    assert_eq!(heights, [("lab", 4), ("services", 6), ("devices", 9)]);
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

fn services_model() -> Shell {
    model(false, false)
}

#[test]
fn a_narrow_key_line_ends_in_a_whole_hint() {
    for build in [services_model as fn() -> Shell, lab_model] {
        let wide = lines(&render(build(), 200, 30)).pop().unwrap();
        let narrow = lines(&render(build(), 60, 15)).pop().unwrap();

        assert!(!narrow.contains('…'), "{narrow}");
        assert!(
            wide.strip_prefix(narrow.as_str())
                .is_some_and(|rest| rest.starts_with("  ")),
            "{narrow}\n{wide}"
        );
    }
}

fn asked(action: &Action) -> Shell {
    let mut model = match action.row {
        Row::Stack => services_model(),
        Row::Device | Row::Node => maintenance_model(false),
        Row::Chapter | Row::Lab => lab_model(),
    };
    press(&mut model, KeyCode::Char(action.key));
    model
}

#[test]
fn the_longest_question_of_the_table_shows_whole_at_60_by_15() {
    let length = |model: &Shell| model.question().map_or(0, |question| question.text().len());
    let longest = ACTIONS.iter().map(asked).max_by_key(length).unwrap();
    let text = longest.question().unwrap().text();

    let screen = lines(&render(longest, 60, 15));

    let start = screen
        .iter()
        .position(|line| line.starts_with(" run nobs"))
        .unwrap();
    let shown = &screen[start..];
    let squeezed = |text: &str| text.split_whitespace().collect::<String>();
    assert!(text.contains(". y/n"), "{text}");
    assert_eq!(
        squeezed(&shown.concat()),
        squeezed(&text),
        "{}",
        screen.join("\n")
    );
    assert!(shown.iter().any(|line| line.ends_with("y/n")));
}

#[test]
fn a_help_that_does_not_fit_says_j_k_more_and_a_help_that_fits_does_not() {
    let opened = lines(&render(pressed('?'), 60, 15)).join("\n");
    let mut lab = lab_model();
    press(&mut lab, KeyCode::Char('?'));
    let fitting = lines(&render(lab, 100, 30)).join("\n");

    assert!(opened.contains("j/k more"), "{opened}");
    assert!(!fitting.contains("j/k more"), "{fitting}");
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

    assert_eq!(workshop.workbench(), Workbench::Observability);
    let screen = lines(&render(workshop, 100, 30));
    assert!(!screen[1].contains("3 lab"), "{}", screen[1]);
    assert!(
        screen[1].starts_with(" 1 devices  2 services  [3 observability]"),
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

fn summarised() -> Vec<Alert> {
    let mut list = alerts();
    list[0].annotations.insert(
        "summary".to_string(),
        "peer 10.1.99.2 is active, expected established".to_string(),
    );
    list
}

fn observed(mut model: Shell, update: Update, key: char) -> Shell {
    model.apply(update);
    press(&mut model, KeyCode::Char(key));
    model
}

fn observability() -> Shell {
    observed(devices_model(), Update::Alerts(Ok(summarised())), '3')
}

fn observability_none() -> Shell {
    let model = devices_model_in(
        PROJECT_LAB,
        "expected/lab_vars.yml",
        "/home/me/nobs-t-project",
    );
    after(model, &[KeyCode::Char('3')])
}

fn observability_down() -> Shell {
    after(workshop_down(), &[KeyCode::Char('3')])
}

fn observability_unknown() -> Shell {
    after(workshop_unpolled(), &[KeyCode::Char('3')])
}

fn observability_no_answer() -> Shell {
    observed(
        devices_model(),
        Update::Alerts(Err("refused".to_string())),
        '3',
    )
}

fn observability_book() -> Shell {
    let mut model = lab_devices_model();
    model.apply(listing(
        0,
        ps(&[("alertmanager", "running", 9093, "40 seconds ago")]),
    ));
    let mut list = summarised();
    for alert in &mut list {
        alert
            .labels
            .insert("device".to_string(), "nobs-t-srl1".to_string());
    }
    observed(model, Update::Alerts(Ok(list)), '4')
}

fn observability_empty() -> Shell {
    observed(devices_model(), Update::Alerts(Ok(Vec::new())), '3')
}

#[test]
fn the_observability_frames_are_pinned() {
    golden("observability", observability);
    golden("observability-none", observability_none);
    pinned(observability_down(), "observability-down", 60, 15);
    pinned(observability_unknown(), "observability-unknown", 60, 15);
    pinned(observability_no_answer(), "observability-no-answer", 60, 15);
    pinned(observability_book(), "observability-book", 60, 15);
    pinned(observability_empty(), "observability-empty", 60, 15);
    pinned(
        after(observability(), &[KeyCode::Char('?')]),
        "help-observability",
        60,
        15,
    );
}

#[test]
fn the_observability_view_names_no_view_kind_or_key_that_is_not_built() {
    let builds: [fn() -> Shell; 7] = [
        observability,
        observability_none,
        observability_down,
        observability_unknown,
        observability_no_answer,
        observability_book,
        observability_empty,
    ];
    for build in builds {
        for (width, height) in SIZES {
            let screen = lines(&render(build(), width, height)).join("\n");
            for word in ["logs", "metrics", "Tab view", "[ ]", "Tab focus"] {
                assert!(!screen.contains(word), "{word}\n{screen}");
            }
        }
    }
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
        .draw(|frame| view::draw_in(frame, area, &sonda, at, None))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let inside: String = (area.y..area.bottom())
        .flat_map(|y| (area.x..area.right()).map(move |x| (x, y)))
        .map(|(x, y)| buffer[(x, y)].symbol())
        .collect();
    assert!(inside.contains("NAME"), "{inside}");
    let stray: Vec<(u16, u16)> = (0..24)
        .flat_map(|y| (0..80).map(move |x| (x, y)))
        .filter(|(x, y)| !area.contains(Position::new(*x, *y)) && buffer[(*x, *y)].symbol() != " ")
        .collect();
    assert!(stray.is_empty(), "{stray:?}");
}

#[derive(Clone, Copy)]
enum Corner {
    Title(&'static str),
    At([(usize, usize); 2]),
}

type State = (&'static str, fn() -> Shell, Corner);

fn after(mut model: Shell, keys: &[KeyCode]) -> Shell {
    for key in keys {
        press(&mut model, *key);
    }
    model
}

fn sonda_only_stack() -> Shell {
    Shell::new(Start {
        chooser: false,
        ..begin(None, vec![sonda_only()], Workbench::Services)
    })
}

fn lab_without_a_chapter() -> Shell {
    let dir = PathBuf::from(BOOK_LAB);
    let lab = locate(&dir, Some(&dir)).unwrap();
    let model = Shell::new(Start {
        chooser: false,
        ..begin(Some(lab), Vec::new(), Workbench::Lab)
    });
    after(model, &[KeyCode::Tab, KeyCode::Tab])
}

#[test]
fn the_box_with_the_focus_draws_the_only_double_border_in_every_state() {
    use Corner::{At, Title};
    use KeyCode::{Char, Enter, Esc, Tab};
    let states: [State; 34] = [
        ("chooser", || model(true, false), Title(" labs found ")),
        (
            "services overview list",
            || model(false, false),
            Title(" stacks "),
        ),
        (
            "services overview tabs",
            || after(model(false, false), &[Tab]),
            Title(" autocon5 "),
        ),
        (
            "services logs tabs",
            || logs_model(false),
            Title(" autocon5 "),
        ),
        (
            "services logs list",
            || after(logs_model(false), &[Tab]),
            Title(" stacks "),
        ),
        (
            "sonda list",
            || after(sonda_model(), &[Esc]),
            Title(" stacks "),
        ),
        ("sonda tabs", sonda_model, At([(24, 5), (33, 5)])),
        (
            "sonda lines",
            || after(sonda_model(), &[Tab]),
            At([(43, 5), (69, 5)]),
        ),
        (
            "sonda without rows",
            || sonda_failed("refused"),
            At([(24, 5), (33, 5)]),
        ),
        (
            "sonda prompt",
            || after(sonda_model(), &[Char('l')]),
            At([(24, 5), (33, 5)]),
        ),
        ("sonda only stack", sonda_only_stack, At([(24, 5), (33, 5)])),
        ("devices list", devices_model, Title(" devices 2 ")),
        (
            "devices without a source",
            without_a_source,
            Title(" devices 0 "),
        ),
        ("devices overview tabs", || on_tab(0), Title(" srl1 ")),
        ("devices interfaces tabs", || on_tab(1), Title(" srl1 ")),
        ("devices timeline tabs", || on_tab(2), Title(" srl1 ")),
        ("devices logs tabs", || on_tab(3), Title(" srl1 ")),
        ("entry box", || after(on_tab(2), &[Enter]), Title(" alert ")),
        (
            "entry box, then tab",
            || after(on_tab(2), &[Enter, Tab]),
            Title(" devices 2 "),
        ),
        ("lab list", lab_model, Title(" chapters ")),
        ("lab tabs", || after(lab_model(), &[Tab]), Title(" nodes ")),
        (
            "lab lines",
            || after(lab_model(), &[Tab, Tab]),
            Title(" ch1 "),
        ),
        (
            "lab without a chapter",
            lab_without_a_chapter,
            At([(23, 2), (42, 2)]),
        ),
        ("keys box", || pressed('?'), Title(" keys ")),
        (
            "lab keys box",
            || after(lab_model(), &[Char('?')]),
            Title(" keys "),
        ),
        (
            "devices keys box",
            || after(devices_model(), &[Char('?')]),
            Title(" keys "),
        ),
        ("error row", || model(false, true), Title(" stacks ")),
        ("run pane open", || pressed('o'), Title(" stacks ")),
        (
            "run pane folded",
            || ran(Some(End::Exited(0)), true),
            Title(" stacks "),
        ),
        (
            "run pane with the focus",
            || focused(ran(Some(End::Exited(0)), false), 100, 30),
            Title(" run "),
        ),
        ("lab run pane open", refused, Title(" chapters ")),
        (
            "lab run pane folded",
            || after(refused(), &[Esc]),
            Title(" chapters "),
        ),
        ("question", || pressed('u'), Title(" stacks ")),
        (
            "filter",
            || after(model(false, false), &[Char('/'), Char('a')]),
            Title(" stacks "),
        ),
    ];
    for (state, build, corner) in states {
        for (size, (width, height)) in SIZES.into_iter().enumerate() {
            let screen = lines(&render(build(), width, height));
            let text = screen.join("\n");
            for symbol in ['╔', '╗', '╚', '╝'] {
                let count = text.matches(symbol).count();
                assert_eq!(count, 1, "{state} {width}x{height} {symbol}\n{text}");
            }
            let (row, line) = screen
                .iter()
                .enumerate()
                .find(|(_, line)| line.contains('╔'))
                .unwrap();
            let at = line.find('╔').unwrap();
            let column = line[..at].chars().count();
            match corner {
                Title(title) => assert!(
                    line[at + '╔'.len_utf8()..].starts_with(title),
                    "{state} {width}x{height} {title}\n{text}"
                ),
                At(places) => assert_eq!(
                    (column, row),
                    places[size],
                    "{state} {width}x{height}\n{text}"
                ),
            }
        }
    }
    for (width, height) in [(59, 15), (60, 14)] {
        let text = lines(&render(model(false, false), width, height)).join("\n");
        assert!(
            !text.contains(['╔', '╗', '╚', '╝']),
            "too small {width}x{height}\n{text}"
        );
    }
}
