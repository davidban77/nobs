use std::ffi::OsString;
use std::fmt::Display;
use std::io::{self, IsTerminal, Write};
use std::panic::{self, PanicHookInfo};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{self, Event, KeyEvent};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute};
use nobs_core::locate::{Kind, Located};
use nobs_core::runner::Runner;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;

use shell::Model;

pub mod actions;
pub mod book;
pub mod data;
pub mod devices;
pub mod poll;
pub mod run;
pub mod screen;
pub mod shell;
pub mod view;

pub use poll::Pollers;
pub use run::{End, FakeLauncher, Launcher, Processes, Running};
pub use screen::draw;
pub use shell::{
    handle, Command, Focus, Model as Shell, Service, SondaTarget, Stack, StackKind, StackState,
    Start, Tab, Update, Workbench,
};

const PERIOD: Duration = Duration::from_secs(1);
const KEY_WAIT: Duration = Duration::from_millis(100);
const DRAIN_LIMIT: Duration = Duration::from_millis(50);

pub fn run(
    start: Start,
    runner: Arc<dyn Runner + Send + Sync>,
    launcher: Arc<dyn Launcher>,
) -> Result<i32, String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("the screen needs a terminal".to_string());
    }
    let sonda = start.sonda.clone();
    let source = start.source.clone();
    let urls = start.lab.is_some().then(|| start.urls.clone());
    let topology = start
        .lab
        .as_ref()
        .filter(|lab| matches!(lab.kind, Kind::BookLab(_)))
        .map(Located::topology);
    let mut model = Model::new(start);
    panic::set_hook(panic_hook(thread::current().id(), panic::take_hook()));
    let screen = Screen::open(io::stdout(), raw_mode)?;
    let (pollers, updates) = Pollers::start(
        runner,
        model.stacks().to_vec(),
        model.selected_stack(),
        sonda,
        source,
        urls,
        topology,
    );
    let mut actions = Actions::new(launcher);
    let result = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(failed)
        .and_then(|mut terminal| {
            turns(
                &mut terminal,
                &mut model,
                &pollers,
                &updates,
                &mut actions,
                read_key,
            )
        });
    drop(screen);
    pollers.send(Command::Quit);
    result
}

struct Actions {
    launcher: Arc<dyn Launcher>,
    sender: Sender<Update>,
    updates: Receiver<Update>,
    running: Option<Box<dyn Running>>,
}

impl Actions {
    fn new(launcher: Arc<dyn Launcher>) -> Actions {
        let (sender, updates) = mpsc::channel();
        Actions {
            launcher,
            sender,
            updates,
            running: None,
        }
    }

    fn start(&mut self, program: &Path, args: &[OsString]) {
        match self.launcher.start(program, args, self.sender.clone()) {
            Ok(running) => self.running = Some(running),
            Err(error) => {
                let _ = self.sender.send(Update::ActionLine(format!(
                    "nobs: cannot run nobs: {error}"
                )));
                let _ = self.sender.send(Update::ActionEnded(End::Exited(1)));
            }
        }
    }

    fn stop(&mut self) {
        if let Some(running) = self.running.as_mut() {
            running.stop();
        }
    }

    fn drain_until_quit(
        &mut self,
        model: &mut Model,
        pollers: &Pollers,
        until: Instant,
    ) -> Option<End> {
        while Instant::now() < until {
            let Ok(update) = self.updates.try_recv() else {
                return None;
            };
            let ended = match update {
                Update::ActionEnded(end) => Some(end),
                _ => None,
            };
            model.apply(update);
            if let Some(end) = ended {
                self.running = None;
                pollers.send(Command::Refresh);
                if model.quit() {
                    return Some(end);
                }
            }
        }
        None
    }
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync>;

fn panic_hook(main: ThreadId, previous: Hook) -> Hook {
    Box::new(move |info| {
        if thread::current().id() == main {
            restore(&mut io::stdout(), &mut raw_mode);
            previous(info);
        }
    })
}

fn turns<B: Backend>(
    terminal: &mut Terminal<B>,
    model: &mut Model,
    pollers: &Pollers,
    updates: &Receiver<Update>,
    actions: &mut Actions,
    mut next_key: impl FnMut(Duration) -> Result<Option<KeyEvent>, String>,
) -> Result<i32, String> {
    let mut gone = false;
    loop {
        let until = Instant::now() + DRAIN_LIMIT;
        drain(updates, model, &mut gone);
        match actions.drain_until_quit(model, pollers, until) {
            Some(End::NotStopped { group }) => {
                return Err(shell::not_stopped(group));
            }
            Some(_) => return Ok(model.exit_code()),
            None => {}
        }
        let size = terminal.size().map_err(failed)?;
        model.tick(Instant::now(), SystemTime::now(), (size.width, size.height));
        terminal.draw(|frame| draw(frame, model)).map_err(failed)?;
        if let Some(key) = next_key(KEY_WAIT)? {
            for command in handle(model, key) {
                match &command {
                    Command::Run { program, args } => actions.start(program, args),
                    Command::Stop => actions.stop(),
                    _ => {}
                }
                pollers.send(command);
            }
        }
        if model.quit() {
            return Ok(model.exit_code());
        }
    }
}

fn drain(updates: &Receiver<Update>, model: &mut Model, gone: &mut bool) {
    let until = Instant::now() + DRAIN_LIMIT;
    while !*gone && Instant::now() < until {
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

fn raw_mode(on: bool) -> io::Result<()> {
    if on {
        enable_raw_mode()
    } else {
        disable_raw_mode()
    }
}

fn restore(out: &mut impl Write, raw: &mut impl FnMut(bool) -> io::Result<()>) {
    let _ = execute!(out, LeaveAlternateScreen, cursor::Show);
    let _ = raw(false);
}

struct Screen<W: Write, R: FnMut(bool) -> io::Result<()>> {
    out: W,
    raw: R,
}

impl<W: Write, R: FnMut(bool) -> io::Result<()>> Screen<W, R> {
    fn open(out: W, mut raw: R) -> Result<Screen<W, R>, String> {
        raw(true).map_err(failed)?;
        let mut screen = Screen { out, raw };
        execute!(screen.out, EnterAlternateScreen).map_err(failed)?;
        Ok(screen)
    }
}

impl<W: Write, R: FnMut(bool) -> io::Result<()>> Drop for Screen<W, R> {
    fn drop(&mut self) {
        restore(&mut self.out, &mut self.raw);
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
    use nobs_core::compose::Compose;
    use nobs_core::lab;
    use nobs_core::locate::{Kind, Located};
    use nobs_core::runner::Output;
    use nobs_core::sot::{self, Device, Maintenance, SourceOfTruth};
    use nobs_core::status::Urls;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::book::NodeRow;
    use crate::data::stub::Stub;
    use crate::shell::RunState;

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
            program: PathBuf::from("nobs"),
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
            None,
        )
    }

    fn actions() -> Actions {
        Actions::new(Arc::new(FakeLauncher::new()))
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
            &mut actions(),
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

        turns(
            &mut terminal,
            &mut model,
            &pollers,
            &updates,
            &mut actions(),
            |_| {
                turn += 1;
                if turn == 1 {
                    sender
                        .send(Update::Services(0, Ok(lab::services(TWO).unwrap())))
                        .unwrap();
                    return Ok(None);
                }
                Ok(Some(q()))
            },
        )
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
            None,
        );

        let result = turns(
            &mut terminal,
            &mut model,
            &pollers,
            &updates,
            &mut actions(),
            |_| Ok(Some(q())),
        );
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
            None,
        );

        let result = turns(
            &mut terminal,
            &mut model,
            &pollers,
            &updates,
            &mut actions(),
            |_| Ok(Some(keys.next().unwrap_or_else(q))),
        );
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
            None,
        );
        let mut terminal = terminal(100, 30);
        let mut gone = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while model.error().is_none_or(|(source, ..)| source != "ps") && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
            drain(&updates, &mut model, &mut gone);
        }

        turns(
            &mut terminal,
            &mut model,
            &pollers,
            &updates,
            &mut actions(),
            |_| Ok(Some(q())),
        )
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
        let (pollers, updates) =
            Pollers::start(runner.clone(), polled, 0, target(), None, None, None);

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
                &mut actions(),
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

        turns(
            &mut terminal,
            &mut model,
            &pollers,
            &updates,
            &mut actions(),
            |_| {
                turn += 1;
                Ok(Some(match turn {
                    1 => key(KeyCode::Char('x')),
                    _ => q(),
                }))
            },
        )
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
            &mut actions(),
            |_| Ok(Some(q())),
        )
        .unwrap();

        let (source, text, _) = model.error().unwrap();
        assert_eq!((source, text), ("poll", "polling stopped"));
    }

    const PAUSED: &str = "paused";

    fn ch(typed: char) -> KeyEvent {
        key(KeyCode::Char(typed))
    }

    fn acting(launcher: &Arc<FakeLauncher>) -> (Model, Actions) {
        let model = Model::new(start(vec![stack(&[CLOSED])], false));
        (model, Actions::new(launcher.clone()))
    }

    fn press(model: &mut Model, actions: &mut Actions, keys: &[KeyEvent]) -> Result<i32, String> {
        let (pollers, updates) = launch(&scripted(Vec::new()), model);
        let mut keys = keys.iter().copied();
        turns(
            &mut terminal(80, 24),
            model,
            &pollers,
            &updates,
            actions,
            |_| keys.next().map(Some).ok_or_else(|| PAUSED.to_string()),
        )
    }

    fn run_state(model: &Model) -> RunState {
        model.run().unwrap().state
    }

    #[test]
    fn y_after_an_action_key_starts_the_command_the_model_built() {
        let launcher = Arc::new(FakeLauncher::new());
        let (mut model, mut actions) = acting(&launcher);

        let result = press(&mut model, &mut actions, &[ch('u'), ch('y')]);

        assert_eq!(result, Err(PAUSED.to_string()));
        assert_eq!(
            launcher.calls(),
            [(PathBuf::from("nobs"), args(&["--dir", "/ws", "ws", "up"]))]
        );
    }

    #[test]
    fn the_lines_of_an_action_show_in_order_while_it_runs_and_after_its_end() {
        let launcher = Arc::new(FakeLauncher::new());
        let (mut model, mut actions) = acting(&launcher);
        press(&mut model, &mut actions, &[ch('u'), ch('y')]).unwrap_err();
        let sender = launcher.sender().unwrap();
        let line = |text: &str| sender.send(Update::ActionLine(text.to_string())).unwrap();

        line("one");
        line("two");
        press(&mut model, &mut actions, &[]).unwrap_err();
        assert_eq!(model.run().unwrap().lines, ["one", "two"]);
        assert_eq!(run_state(&model), RunState::Running);

        sender.send(Update::ActionEnded(End::Exited(0))).unwrap();
        press(&mut model, &mut actions, &[]).unwrap_err();
        assert_eq!(run_state(&model), RunState::Ended(End::Exited(0)));

        line("three");
        press(&mut model, &mut actions, &[]).unwrap_err();
        assert_eq!(model.run().unwrap().lines, ["one", "two", "three"]);
        assert_eq!(run_state(&model), RunState::Ended(End::Exited(0)));
    }

    #[test]
    fn ctrl_c_stops_the_action_once_and_the_next_key_is_still_handled() {
        let launcher = Arc::new(FakeLauncher::new());
        let (mut model, mut actions) = acting(&launcher);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        press(&mut model, &mut actions, &[ch('u'), ch('y'), ctrl_c]).unwrap_err();

        assert_eq!(launcher.stops(), 1);
        assert_eq!(run_state(&model), RunState::Ended(End::Stopped));
        assert_eq!(press(&mut model, &mut actions, &[q()]), Ok(0));
    }

    #[test]
    fn q_and_y_while_an_action_runs_stop_it_before_the_loop_returns() {
        let launcher = Arc::new(FakeLauncher::new());
        let (mut model, mut actions) = acting(&launcher);

        let result = press(&mut model, &mut actions, &[ch('u'), ch('y'), q(), ch('y')]);

        assert_eq!(result, Ok(0));
        assert_eq!(launcher.stops(), 1);
    }

    #[test]
    fn an_action_that_did_not_stop_after_q_and_y_makes_the_loop_fail_with_its_group() {
        let launcher = Arc::new(FakeLauncher::new());
        let (mut model, mut actions) = acting(&launcher);
        press(&mut model, &mut actions, &[ch('u'), ch('y')]).unwrap_err();
        let sender = launcher.sender().unwrap();
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        let mut keys = [q(), ch('y')].into_iter();

        let result = turns(
            &mut terminal(80, 24),
            &mut model,
            &pollers,
            &updates,
            &mut actions,
            |_| {
                let key = keys.next().ok_or_else(|| PAUSED.to_string())?;
                if key == ch('y') {
                    sender
                        .send(Update::ActionEnded(End::NotStopped { group: 4242 }))
                        .unwrap();
                }
                Ok(Some(key))
            },
        );

        assert_eq!(
            result,
            Err("the action did not stop: a program it started is still running in process group 4242".to_string())
        );
    }

    #[test]
    fn a_command_that_cannot_start_ends_with_its_error_and_exit_1() {
        let launcher = Arc::new(FakeLauncher::failing());
        let (mut model, mut actions) = acting(&launcher);

        press(&mut model, &mut actions, &[ch('u'), ch('y')]).unwrap_err();

        assert_eq!(
            model.run().unwrap().lines,
            ["nobs: cannot run nobs: fake launcher failure"]
        );
        assert_eq!(run_state(&model), RunState::Ended(End::Exited(1)));
    }

    #[test]
    fn a_command_that_prints_without_end_never_delays_a_key() {
        let launcher = Arc::new(FakeLauncher::new());
        let (mut model, mut actions) = acting(&launcher);
        press(&mut model, &mut actions, &[ch('u'), ch('y')]).unwrap_err();
        let mut flood = launcher.sender();
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        let mut keys = [ch('j'), ch('k'), q(), ch('y')].into_iter();
        let mut sent: Option<Instant> = None;
        let mut gaps = Vec::new();

        let result = turns(
            &mut terminal(80, 24),
            &mut model,
            &pollers,
            &updates,
            &mut actions,
            |_| {
                if let Some(at) = sent {
                    gaps.push(at.elapsed());
                }
                if let Some(sender) = flood.take() {
                    thread::spawn(move || {
                        let end = Instant::now() + Duration::from_secs(1);
                        while Instant::now() < end
                            && sender.send(Update::ActionLine("more".to_string())).is_ok()
                        {
                        }
                    });
                }
                let key = keys.next().ok_or_else(|| PAUSED.to_string())?;
                sent = Some(Instant::now());
                Ok(Some(key))
            },
        );

        assert!(
            gaps.iter().all(|gap| *gap < Duration::from_millis(100)),
            "{gaps:?}"
        );
        assert_eq!(result, Err(PAUSED.to_string()));
        assert_eq!(gaps.len(), 4);
        assert_eq!(launcher.stops(), 1);
    }

    #[test]
    fn closing_the_screen_leaves_the_alternate_screen_and_turns_raw_mode_off() {
        let mut written = Vec::new();
        let mut raw = Vec::new();

        drop(
            Screen::open(&mut written, |on| {
                raw.push(on);
                Ok(())
            })
            .unwrap(),
        );

        let mut leave = Vec::new();
        execute!(leave, LeaveAlternateScreen).unwrap();
        assert!(
            written.windows(leave.len()).any(|part| part == leave),
            "{:?}",
            String::from_utf8_lossy(&written)
        );
        assert_eq!(raw, [true, false]);
    }

    #[test]
    fn the_nodes_thread_gives_each_node_the_state_of_its_container() {
        let runner = scripted(vec![answer(
            0,
            "clab-nobs-t-book-nobs-t-srl1\texited\ngrafana\trunning\n",
            "",
        )]);
        let topology = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/book-lab/containerlab/lab.yml");
        let (_pollers, updates) = Pollers::start(
            runner.clone(),
            Vec::new(),
            0,
            target(),
            None,
            None,
            Some(topology),
        );

        let rows = next(&updates, Duration::from_secs(5), |update| match update {
            Update::Nodes(result) => Some(result),
            _ => None,
        });

        let row = |name: &str, state: Option<&str>| NodeRow {
            name: name.to_string(),
            kind: "nokia_srlinux".to_string(),
            state: state.map(str::to_string),
        };
        assert_eq!(
            rows,
            Ok(vec![
                row("nobs-t-srl1", Some("exited")),
                row("nobs-t-srl2", None)
            ])
        );
        assert_eq!(
            runner.calls(),
            [(
                Compose::PROGRAM.to_string(),
                args(&["ps", "-a", "--format", "{{.Names}}\t{{.State}}"])
            )]
        );
    }

    #[test]
    fn the_ps_of_a_book_lab_chapter_leaves_out_the_other_chapters_and_a_workshop_keeps_them() {
        let runner = scripted(Vec::new());
        let chapter = Stack {
            name: "ch1".to_string(),
            kind: StackKind::BookLab,
            ..stack(&[CLOSED])
        };
        let (_pollers, updates) = Pollers::start(
            runner.clone(),
            vec![stack(&[CLOSED]), chapter],
            0,
            target(),
            None,
            None,
            None,
        );

        next(&updates, Duration::from_secs(5), |update| {
            matches!(update, Update::Services(1, _)).then_some(())
        });

        let ps = |extra: &[&str]| {
            let mut words = vec![
                "compose",
                "--project-name",
                "p",
                "-f",
                "/ws/compose.yml",
                "ps",
                "--all",
            ];
            words.extend(extra);
            words.extend(["--format", "json"]);
            (Compose::PROGRAM.to_string(), args(&words))
        };
        assert_eq!(runner.calls()[..2], [ps(&[]), ps(&["--orphans=false"])]);
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
