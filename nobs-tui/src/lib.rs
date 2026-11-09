use std::fmt::Display;
use std::io::{self, IsTerminal};
use std::panic::{self, PanicHookInfo};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{self, Event, KeyEvent};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute};
use nobs_core::runner::Runner;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;

use shell::Model;

pub mod data;
pub mod devices;
pub mod poll;
pub mod screen;
pub mod shell;
pub mod view;

pub use poll::Pollers;
pub use screen::draw;
pub use shell::{
    handle, Command, Focus, Model as Shell, Service, SondaTarget, Stack, StackKind, StackState,
    Start, Tab, Update, Workbench,
};

const PERIOD: Duration = Duration::from_secs(1);
const KEY_WAIT: Duration = Duration::from_millis(100);

pub fn run(start: Start, runner: Arc<dyn Runner + Send + Sync>) -> Result<i32, String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("the screen needs a terminal".to_string());
    }
    let sonda = start.sonda.clone();
    let source = start.source.clone();
    let urls = start.lab.is_some().then(|| start.urls.clone());
    let mut model = Model::new(start);
    panic::set_hook(panic_hook(thread::current().id(), panic::take_hook()));
    let screen = Screen::open()?;
    let (pollers, updates) = Pollers::start(
        runner,
        model.stacks().to_vec(),
        model.selected_stack(),
        sonda,
        source,
        urls,
    );
    let result = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(failed)
        .and_then(|mut terminal| turns(&mut terminal, &mut model, &pollers, &updates, read_key));
    drop(screen);
    pollers.send(Command::Quit);
    result
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync>;

fn panic_hook(main: ThreadId, previous: Hook) -> Hook {
    Box::new(move |info| {
        if thread::current().id() == main {
            restore();
            previous(info);
        }
    })
}

fn turns<B: Backend>(
    terminal: &mut Terminal<B>,
    model: &mut Model,
    pollers: &Pollers,
    updates: &Receiver<Update>,
    mut next_key: impl FnMut(Duration) -> Result<Option<KeyEvent>, String>,
) -> Result<i32, String> {
    let mut gone = false;
    loop {
        drain(updates, model, &mut gone);
        let size = terminal.size().map_err(failed)?;
        model.tick(Instant::now(), SystemTime::now(), (size.width, size.height));
        terminal.draw(|frame| draw(frame, model)).map_err(failed)?;
        if let Some(key) = next_key(KEY_WAIT)? {
            for command in handle(model, key) {
                pollers.send(command);
            }
        }
        if model.quit() {
            return Ok(model.exit_code());
        }
    }
}

fn drain(updates: &Receiver<Update>, model: &mut Model, gone: &mut bool) {
    while !*gone {
        match updates.try_recv() {
            Ok(update) => model.apply(update),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                *gone = true;
                model.apply(Update::Stopped("poll".to_string()));
            }
        }
    }
}

fn read_key(wait: Duration) -> Result<Option<KeyEvent>, String> {
    if !event::poll(wait).map_err(failed)? {
        return Ok(None);
    }
    match event::read().map_err(failed)? {
        Event::Key(key) => Ok(Some(key)),
        _ => Ok(None),
    }
}

fn failed(error: impl Display) -> String {
    format!("the screen failed: {error}")
}

fn restore() {
    let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
}

struct Screen;

impl Screen {
    fn open() -> Result<Screen, String> {
        enable_raw_mode().map_err(failed)?;
        let screen = Screen;
        execute!(io::stdout(), EnterAlternateScreen).map_err(failed)?;
        Ok(screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::sync::Mutex;

    use crossterm::event::{KeyCode, KeyModifiers};
    use nobs_core::lab;
    use nobs_core::locate::{Kind, Located};
    use nobs_core::runner::Output;
    use nobs_core::sot::{self, Device, Maintenance, SourceOfTruth};
    use nobs_core::status::Urls;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::data::stub::Stub;

    const CLOSED: (&str, &str) = ("SONDA_SERVER_URL", "http://127.0.0.1:1");
    const TWO: &str = "{\"Service\":\"grafana\",\"State\":\"running\"}\n{\"Service\":\"loki\",\"State\":\"exited\"}\n";
    const LOKI: &str = "{\"Service\":\"loki\",\"State\":\"running\"}\n";
    const DOCKER_DOWN: &str = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock";

    #[derive(Default)]
    struct Scripted {
        calls: Mutex<Vec<(String, Vec<OsString>)>>,
        answers: Mutex<VecDeque<Output>>,
        sleep: Duration,
    }

    impl Runner for Scripted {
        fn run(&self, _program: &str, _args: &[OsString]) -> io::Result<i32> {
            Ok(0)
        }

        fn output(&self, program: &str, args: &[OsString]) -> io::Result<Output> {
            self.calls
                .lock()
                .unwrap()
                .push((program.to_string(), args.to_vec()));
            thread::sleep(self.sleep);
            Ok(self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| answer(0, "", "")))
        }
    }

    struct Panicking;

    impl Runner for Panicking {
        fn run(&self, _program: &str, _args: &[OsString]) -> io::Result<i32> {
            Ok(0)
        }

        fn output(&self, _program: &str, _args: &[OsString]) -> io::Result<Output> {
            panic!("the runner broke");
        }
    }

    impl Scripted {
        fn calls(&self) -> Vec<(String, Vec<OsString>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn scripted(answers: Vec<Output>) -> Arc<Scripted> {
        Arc::new(Scripted {
            answers: Mutex::new(answers.into()),
            ..Scripted::default()
        })
    }

    fn answer(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn q() -> KeyEvent {
        key(KeyCode::Char('q'))
    }

    fn stack(env: &[(&str, &str)]) -> Stack {
        let path = PathBuf::from("/ws");
        Stack {
            name: "ws".to_string(),
            kind: StackKind::Workshop,
            compose: path.join("compose.yml"),
            path,
            project: Some("p".to_string()),
            env: env
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        }
    }

    fn target() -> SondaTarget {
        SondaTarget {
            server: None,
            api_key: None,
        }
    }

    fn start(stacks: Vec<Stack>, chooser: bool) -> Start {
        let lab = (!chooser).then(|| Located {
            dir: stacks[0].path.clone(),
            kind: Kind::BookLab(None),
        });
        Start {
            lab,
            stacks,
            sonda: target(),
            home: Workbench::Services,
            chooser,
            source: None,
            source_error: None,
            urls: Urls::defaults(),
        }
    }

    fn launch(runner: &Arc<Scripted>, model: &Model) -> (Pollers, Receiver<Update>) {
        Pollers::start(
            runner.clone(),
            model.stacks().to_vec(),
            model.selected_stack(),
            target(),
            None,
            None,
        )
    }

    fn terminal(width: u16, height: u16) -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(width, height)).unwrap()
    }

    fn text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        buffer.content().iter().map(|cell| cell.symbol()).collect()
    }

    fn next<T>(
        updates: &Receiver<Update>,
        limit: Duration,
        mut pick: impl FnMut(Update) -> Option<T>,
    ) -> T {
        let end = Instant::now() + limit;
        loop {
            let update = updates
                .recv_timeout(end.saturating_duration_since(Instant::now()))
                .expect("an update in time");
            if let Some(found) = pick(update) {
                return found;
            }
        }
    }

    fn services(updates: &Receiver<Update>) -> Result<Vec<lab::Service>, String> {
        next(updates, Duration::from_secs(5), |update| match update {
            Update::Services(0, result) => Some(result),
            _ => None,
        })
    }

    #[test]
    fn each_turn_waits_100_ms_for_a_key() {
        let mut model = Model::new(start(vec![stack(&[CLOSED])], false));
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        let mut waits = Vec::new();

        turns(
            &mut terminal(80, 24),
            &mut model,
            &pollers,
            &updates,
            |wait| {
                waits.push(wait);
                Ok((waits.len() == 2).then(q))
            },
        )
        .unwrap();

        assert_eq!(waits, [Duration::from_millis(100); 2]);
    }

    #[test]
    fn an_update_sent_during_a_turn_is_drawn_on_the_next_turn() {
        let mut model = Model::new(start(vec![stack(&[CLOSED])], false));
        let (pollers, _updates) = launch(&scripted(Vec::new()), &model);
        let (sender, updates) = mpsc::channel();
        let mut terminal = terminal(80, 24);
        let mut turn = 0;

        turns(&mut terminal, &mut model, &pollers, &updates, |_| {
            turn += 1;
            if turn == 1 {
                sender
                    .send(Update::Services(0, Ok(lab::services(TWO).unwrap())))
                    .unwrap();
                return Ok(None);
            }
            Ok(Some(q()))
        })
        .unwrap();

        let screen = text(&terminal);
        assert!(screen.contains("grafana"), "{screen}");
        assert!(screen.contains("loki"), "{screen}");
    }

    #[test]
    fn a_program_and_a_server_that_hold_on_never_delay_a_key_or_the_frame() {
        let stub = Stub::start_silent(Duration::from_secs(2));
        let url = stub.url();
        let env = [
            ("SONDA_SERVER_URL", url),
            ("PROMETHEUS_URL", url),
            ("LOKI_URL", url),
            ("ALERTMANAGER_URL", url),
            ("INFRAHUB_ADDRESS", url),
        ];
        let mut model = Model::new(start(vec![stack(&env)], false));
        model.tick(Instant::now(), SystemTime::now(), (100, 30));
        model.apply(Update::Services(0, Ok(lab::services(TWO).unwrap())));
        let runner = Arc::new(Scripted {
            sleep: Duration::from_secs(3),
            ..Scripted::default()
        });
        let mut terminal = terminal(100, 30);
        let started = Instant::now();
        let (pollers, updates) = Pollers::start(
            runner,
            model.stacks().to_vec(),
            model.selected_stack(),
            target(),
            None,
            None,
        );

        let result = turns(&mut terminal, &mut model, &pollers, &updates, |_| {
            Ok(Some(q()))
        });
        let elapsed = started.elapsed();

        assert_eq!(result, Ok(0));
        assert!(elapsed < Duration::from_millis(100), "{elapsed:?}");
        let screen = text(&terminal);
        assert!(screen.contains("refreshed"), "{screen}");
        assert!(screen.contains("grafana"), "{screen}");
        assert!(screen.contains("loki"), "{screen}");
    }

    struct Holding;

    impl SourceOfTruth for Holding {
        fn devices(&self) -> Result<Vec<Device>, sot::Error> {
            thread::sleep(Duration::from_secs(2));
            Ok(Vec::new())
        }

        fn device(&self, _name: &str) -> Result<Option<Device>, sot::Error> {
            Ok(None)
        }

        fn set_maintenance(&self, _device: &str, _state: bool) -> Result<Maintenance, sot::Error> {
            Ok(Maintenance { previous: false })
        }
    }

    fn device(name: &str) -> Device {
        Device {
            name: name.to_string(),
            kind: None,
            site: None,
            role: None,
            asn: None,
            maintenance: false,
            interfaces: Vec::new(),
            sessions: Vec::new(),
        }
    }

    #[test]
    fn the_device_sources_that_hold_on_never_delay_a_key() {
        let stub = Stub::start_silent(Duration::from_secs(2));
        let address = || stub.url().to_string();
        let begin = Start {
            home: Workbench::Devices,
            source: Some(Arc::new(Holding)),
            urls: Urls {
                grafana: address(),
                prometheus: address(),
                loki: address(),
                alertmanager: address(),
                infrahub: address(),
                prefect: address(),
                sonda: address(),
            },
            ..start(vec![stack(&[CLOSED])], false)
        };
        let (source, urls) = (begin.source.clone(), Some(begin.urls.clone()));
        let mut model = Model::new(begin);
        model.tick(Instant::now(), SystemTime::now(), (100, 30));
        model.apply(Update::Devices(Ok(vec![device("srl1")])));
        let mut terminal = terminal(100, 30);
        let mut keys = vec![
            key(KeyCode::Enter),
            key(KeyCode::Char(']')),
            key(KeyCode::Char(']')),
        ]
        .into_iter();
        let started = Instant::now();
        let (pollers, updates) = Pollers::start(
            scripted(Vec::new()),
            model.stacks().to_vec(),
            model.selected_stack(),
            target(),
            source,
            urls,
        );

        let result = turns(&mut terminal, &mut model, &pollers, &updates, |_| {
            Ok(Some(keys.next().unwrap_or_else(q)))
        });
        let elapsed = started.elapsed();

        assert_eq!(result, Ok(0));
        assert_eq!(model.tab(), Tab::Timeline);
        assert!(elapsed < Duration::from_millis(100), "{elapsed:?}");
        let screen = text(&terminal);
        assert!(screen.contains("devices"), "{screen}");
    }

    #[test]
    fn a_panic_off_the_main_thread_writes_nothing_and_the_screen_shows_the_stopped_row() {
        let (sender, receiver) = mpsc::channel();
        panic::set_hook(panic_hook(
            thread::current().id(),
            Box::new(move |_| sender.send(()).unwrap()),
        ));

        let joined = thread::spawn(|| panic!("boom")).join();
        let _ = panic::take_hook();

        assert!(joined.is_err());
        assert!(receiver.try_recv().is_err());
        let mut model = Model::new(start(vec![stack(&[CLOSED])], false));
        let (pollers, updates) = Pollers::start(
            Arc::new(Panicking),
            model.stacks().to_vec(),
            model.selected_stack(),
            target(),
            None,
            None,
        );
        let mut terminal = terminal(100, 30);
        let mut turn = 0;

        turns(&mut terminal, &mut model, &pollers, &updates, |_| {
            turn += 1;
            thread::sleep(Duration::from_millis(20));
            Ok((turn == 25).then(q))
        })
        .unwrap();

        let screen = text(&terminal);
        assert!(screen.contains("ps: polling stopped"), "{screen}");
        assert!(screen.contains("nobs"), "{screen}");
    }

    #[test]
    fn following_the_logs_asks_for_what_is_new_and_shows_each_line_once() {
        let runner = scripted(vec![
            answer(
                0,
                "2026-11-08T13:42:01.000000000Z one\n2026-11-08T13:42:02.000000000Z two\n",
                "",
            ),
            answer(
                0,
                "2026-11-08T13:42:02.000000000Z two\n2026-11-08T13:42:03.000000000Z three\n",
                "",
            ),
        ]);
        let mut model = Model::new(start(vec![stack(&[CLOSED])], false));
        model.apply(Update::Services(0, Ok(lab::services(LOKI).unwrap())));
        let mut polled = model.stacks().to_vec();
        polled[0].kind = StackKind::Sonda;
        let (pollers, updates) = Pollers::start(runner.clone(), polled, 0, target(), None, None);

        for command in [KeyCode::Tab, KeyCode::Char('l')]
            .into_iter()
            .flat_map(|code| handle(&mut model, key(code)))
        {
            pollers.send(command);
        }
        pollers.send(Command::Refresh);
        for _ in 0..2 {
            let update = next(&updates, Duration::from_secs(5), |update| {
                matches!(update, Update::Logs(..)).then_some(update)
            });
            model.apply(update);
        }
        pollers.send(Command::Follow(false));
        thread::sleep(Duration::from_secs(3));

        let base = [
            "compose",
            "--project-name",
            "p",
            "-f",
            "/ws/compose.yml",
            "logs",
            "--no-log-prefix",
            "--timestamps",
        ];
        let ask = |tail: &str| {
            let mut words = base.to_vec();
            words.extend([tail, "loki"]);
            ("docker".to_string(), args(&words))
        };
        assert_eq!(
            runner.calls(),
            [
                ask("--tail=200"),
                ask("--since=2026-11-08T13:42:02.000000000Z")
            ]
        );
        let (_, lines, _, _) = model.logs().unwrap();
        assert_eq!(lines, ["one", "two", "three"]);
    }

    #[test]
    fn ps_answers_become_rows_or_the_last_line_of_the_error() {
        let runner = scripted(vec![
            answer(0, TWO, ""),
            answer(1, "", &format!("{DOCKER_DOWN}\n")),
        ]);
        let model = Model::new(start(vec![stack(&[CLOSED])], false));
        let (pollers, updates) = launch(&runner, &model);

        let rows = services(&updates).unwrap();
        pollers.send(Command::Refresh);
        let failure = services(&updates);

        let names: Vec<(&str, &str)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.state.as_str()))
            .collect();
        assert_eq!(names, [("grafana", "running"), ("loki", "exited")]);
        assert_eq!(failure, Err(DOCKER_DOWN.to_string()));
        let asked = (
            "docker".to_string(),
            args(&[
                "compose",
                "--project-name",
                "p",
                "-f",
                "/ws/compose.yml",
                "ps",
                "--all",
                "--format",
                "json",
            ]),
        );
        assert!(runner.calls().iter().all(|call| *call == asked));
    }

    #[test]
    fn health_shows_ok_and_the_http_code_and_down_for_the_rest() {
        let stub = Stub::start(&[("GET", "/-/ready", 200, ""), ("GET", "/ready", 503, "")]);
        let url = stub.url();
        let env = [CLOSED, ("PROMETHEUS_URL", url), ("LOKI_URL", url)];
        let model = Model::new(start(vec![stack(&env)], false));
        let (_pollers, updates) = launch(&scripted(Vec::new()), &model);

        let health = next(&updates, Duration::from_secs(10), |update| match update {
            Update::Health(0, list) => Some(list),
            _ => None,
        });

        let state = |name: &str| {
            health
                .iter()
                .find(|entry| entry.service == name)
                .map(|entry| entry.state.as_str())
        };
        assert_eq!(state("prometheus"), Some("ok"));
        assert_eq!(state("loki"), Some("http 503"));
        assert_eq!(state("grafana"), Some("down"));
    }

    #[test]
    fn a_thread_that_dies_tells_the_screen_it_stopped() {
        let (_pollers, updates) = Pollers::start(
            Arc::new(Panicking),
            vec![stack(&[CLOSED])],
            0,
            target(),
            None,
            None,
        );

        let source = next(&updates, Duration::from_secs(2), |update| match update {
            Update::Stopped(source) => Some(source),
            _ => None,
        });

        assert_eq!(source, "ps");
    }

    #[test]
    fn the_loop_returns_1_from_the_chooser_and_0_from_a_lab() {
        for (chooser, code) in [(true, 1), (false, 0)] {
            let mut model = Model::new(start(vec![stack(&[CLOSED])], chooser));
            let (pollers, updates) = launch(&scripted(Vec::new()), &model);

            let result = turns(
                &mut terminal(80, 24),
                &mut model,
                &pollers,
                &updates,
                |_| Ok(Some(q())),
            );

            assert_eq!(result, Ok(code), "chooser {chooser}");
        }
    }

    #[test]
    fn the_sonda_row_of_the_chooser_polls_the_address_its_own_stack_names() {
        let stub = Stub::start(&[("GET", "/scenarios", 200, r#"{"scenarios":[]}"#)]);
        let sonda = Stack {
            name: "sonda only".to_string(),
            kind: StackKind::Sonda,
            env: vec![("SONDA_SERVER_URL".to_string(), stub.url().to_string())],
            ..stack(&[])
        };
        let mut model = Model::new(start(vec![stack(&[CLOSED]), sonda], true));
        let (pollers, _updates) = launch(&scripted(Vec::new()), &model);

        let commands: Vec<Command> = [KeyCode::Char('j'), KeyCode::Enter]
            .into_iter()
            .flat_map(|code| handle(&mut model, key(code)))
            .collect();
        for command in &commands {
            pollers.send(command.clone());
        }

        assert_eq!(commands, [Command::SelectStack(1)]);
        let listed = || stub.requests().iter().any(|seen| seen.path == "/scenarios");
        let end = Instant::now() + Duration::from_secs(2);
        while !listed() && Instant::now() < end {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(listed(), "{:?}", stub.requests());
        assert_eq!(model.sonda().server(), stub.url());
    }

    #[test]
    fn a_dead_update_channel_shows_polling_stopped_once() {
        let mut model = Model::new(start(vec![stack(&[CLOSED])], false));
        let (pollers, _updates) = launch(&scripted(Vec::new()), &model);
        let (sender, updates) = mpsc::channel();
        drop(sender);
        let mut terminal = terminal(80, 24);
        let mut turn = 0;

        turns(&mut terminal, &mut model, &pollers, &updates, |_| {
            turn += 1;
            Ok(Some(match turn {
                1 => key(KeyCode::Char('x')),
                _ => q(),
            }))
        })
        .unwrap();

        assert_eq!(turn, 2);
        assert!(model.error().is_none());
    }

    #[test]
    fn a_dead_update_channel_shows_polling_stopped() {
        let mut model = Model::new(start(vec![stack(&[CLOSED])], false));
        let (pollers, _updates) = launch(&scripted(Vec::new()), &model);
        let (sender, updates) = mpsc::channel();
        drop(sender);

        turns(
            &mut terminal(80, 24),
            &mut model,
            &pollers,
            &updates,
            |_| Ok(Some(q())),
        )
        .unwrap();

        let (source, text, _) = model.error().unwrap();
        assert_eq!((source, text), ("poll", "polling stopped"));
    }

    #[test]
    fn the_clock_is_a_time_of_day() {
        let mut model = Model::new(start(vec![stack(&[CLOSED])], false));
        model.tick(Instant::now(), SystemTime::now(), (80, 24));
        let now = model.clock().to_string();
        let parts: Vec<u32> = now.split(':').map(|part| part.parse().unwrap()).collect();
        assert_eq!(now.len(), 8, "{now}");
        assert!(parts[0] < 24 && parts[1] < 60 && parts[2] < 60, "{now}");
    }
}
