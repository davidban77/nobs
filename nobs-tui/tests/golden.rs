use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nobs_core::lab::{services, Service};
use nobs_tui::shell::Health;
use nobs_tui::{
    data, draw, handle, view, Shell, SondaTarget, Stack, StackKind, Start, Update, Workbench,
};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

const CLOCK: &str = "13:42:07";
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

fn model(chooser: bool, packt_failed: bool) -> Shell {
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
        lab: None,
        stacks,
        sonda: SondaTarget {
            server: None,
            api_key: None,
        },
        home: Workbench::Services,
        chooser: true,
    });
    if !chooser {
        press(&mut model, KeyCode::Enter);
    }
    model.tick(start, CLOCK.to_string(), (80, 24));
    if packt_failed {
        model.apply(Update::Services(1, Err("exit 1".to_string())));
        model.tick(start + Duration::from_secs(10), CLOCK.to_string(), (80, 24));
    }
    model.apply(Update::Services(0, Ok(ps(&WORKSHOP))));
    model.apply(Update::Health(0, health()));
    if !packt_failed {
        model.apply(Update::Services(1, Ok(Vec::new())));
    }
    model.apply(Update::Services(2, Ok(ps(&DEMO))));
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

fn render(mut model: Shell, width: u16, height: u16) -> Buffer {
    let now = model.now() + Duration::from_secs(2);
    model.tick(now, CLOCK.to_string(), (width, height));
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
fn the_placeholder_help_frame_is_pinned() {
    let mut model = pressed('1');
    press(&mut model, KeyCode::Char('?'));
    let actual = lines(&render(model, 60, 15));

    let failure = mismatch("help-devices", 60, 15, &actual);

    assert!(failure.is_none(), "{}", failure.unwrap_or_default());
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
    golden("devices", || pressed('1'));
}

#[test]
fn the_lab_frame_is_pinned() {
    golden("lab", || pressed('3'));
}

#[test]
fn the_run_pane_frame_is_pinned() {
    golden("run-pane", || pressed('o'));
}

#[test]
fn the_too_small_frame_is_pinned() {
    let actual = lines(&render(model(false, false), 59, 15));

    let failure = mismatch("too-small", 59, 15, &actual);

    assert!(failure.is_none(), "{}", failure.unwrap_or_default());
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
