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
use nobs_core::runner::Runner;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::Terminal;

use aside::ask_aside;
use poll::LabFiles;
use run::Invocation;
use shell::Model;

pub mod actions;
mod aside;
pub mod book;
pub mod data;
pub mod devices;
mod keys;
pub mod poll;
mod pty;
pub mod run;
pub mod screen;
pub mod shell;
mod terminal;
pub mod view;

pub use aside::Asker;
pub use poll::Pollers;
pub use pty::{Ptys, Terminals, Wire};
pub use run::{End, Launcher, Processes, Running};
pub use screen::draw;
pub use shell::{
    handle, Command, Focus, Model as Shell, Service, SondaTarget, Stack, StackKind, StackState,
    Start, Tab, Update, Workbench,
};

const PERIOD: Duration = Duration::from_secs(1);
const KEY_WAIT: Duration = Duration::from_millis(100);
const DRAIN_LIMIT: Duration = Duration::from_millis(50);
const TYPE_WAIT: Duration = Duration::from_millis(20);
const TYPE_DRAIN: Duration = Duration::from_millis(10);

pub fn run(
    start: Start,
    runner: Arc<dyn Runner + Send + Sync>,
    launcher: Arc<dyn Launcher>,
    terminals: Arc<dyn Terminals>,
    asker: Arc<dyn Asker>,
) -> Result<i32, String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("the screen needs a terminal".to_string());
    }
    panic::set_hook(panic_hook(thread::current().id(), panic::take_hook()));
    let mut screen = Screen::open(io::stdout(), raw_mode)?;
    let (mut model, pollers, updates) = started(start, runner);
    let mut actions = Actions::new(launcher, asker);
    let mut wires = Wires::new(terminals);
    let result = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(failed)
        .and_then(|mut terminal| {
            turns(
                &mut Console {
                    terminal: &mut terminal,
                    screen: &mut screen,
                },
                &mut model,
                &mut Workers {
                    pollers: &pollers,
                    updates: &updates,
                    actions: &mut actions,
                    wires: &mut wires,
                },
                read_key,
            )
        });
    drop(screen);
    pollers.send(Command::Quit);
    result
}

fn started(
    start: Start,
    runner: Arc<dyn Runner + Send + Sync>,
) -> (Model, Pollers, Receiver<Update>) {
    let sonda = start.sonda.clone();
    let source = start
        .source
        .clone()
        .map(|source| (source, start.source_service));
    let urls = start.lab.is_some().then(|| start.urls.clone());
    let lab = start
        .lab
        .as_ref()
        .and_then(|lab| LabFiles::of(lab, start.nodes, start.topology.clone()));
    let model = Model::new(start);
    let (pollers, updates) = Pollers::start(
        runner,
        model.stacks().to_vec(),
        model.selected_stack(),
        sonda,
        source,
        urls,
        lab,
    );
    (model, pollers, updates)
}

struct Actions {
    launcher: Arc<dyn Launcher>,
    sender: Sender<Update>,
    updates: Receiver<Update>,
    running: Option<Box<dyn Running>>,
    asker: Arc<dyn Asker>,
    asking: Option<Vec<String>>,
}

impl Actions {
    fn new(launcher: Arc<dyn Launcher>, asker: Arc<dyn Asker>) -> Actions {
        let (sender, updates) = mpsc::channel();
        Actions {
            launcher,
            sender,
            updates,
            running: None,
            asker,
            asking: None,
        }
    }

    fn start(&mut self, child: Invocation<'_>) {
        match self.launcher.start(child, self.sender.clone()) {
            Ok(running) => self.running = Some(running),
            Err(error) => {
                let _ = self.sender.send(Update::ActionLine(cannot_run(error)));
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
        wires: &mut Wires,
        pollers: &Pollers,
        until: Instant,
    ) -> Option<End> {
        while let Ok(update) = next_before(&self.updates, until) {
            let ended = match update {
                Update::ActionEnded(end) => Some(end),
                _ => None,
            };
            send(model.apply(update), self, wires, pollers);
            if let Some(end) = ended {
                self.running = None;
                if self.asking.is_some() {
                    return None;
                }
                pollers.send(Command::Refresh);
                if model.quit() {
                    return Some(end);
                }
            }
        }
        None
    }
}

fn cannot_run(error: io::Error) -> String {
    format!("nobs: cannot run nobs: {error}")
}

struct Wires {
    terminals: Arc<dyn Terminals>,
    sender: Sender<Update>,
    updates: Receiver<Update>,
    open: Vec<(u64, Box<dyn Wire>)>,
}

impl Wires {
    fn new(terminals: Arc<dyn Terminals>) -> Wires {
        let (sender, updates) = mpsc::channel();
        Wires {
            terminals,
            sender,
            updates,
            open: Vec::new(),
        }
    }

    fn open(&mut self, id: u64, child: Invocation<'_>, dir: &Path, size: (u16, u16)) {
        let opened = self
            .terminals
            .open(child, dir, size, id, self.sender.clone());
        match opened {
            Ok(wire) => self.open.push((id, wire)),
            Err(error) => {
                let line = cannot_run(error).into_bytes();
                let _ = self.sender.send(Update::TermOutput(id, line));
                let _ = self.sender.send(Update::TermEnded(id, 1));
            }
        }
    }

    fn wire(&mut self, id: u64) -> Option<&mut Box<dyn Wire>> {
        self.open
            .iter_mut()
            .find(|(open, _)| *open == id)
            .map(|(_, wire)| wire)
    }

    fn close(&mut self, id: u64) {
        if let Some(wire) = self.wire(id) {
            wire.close();
        }
        self.open.retain(|(open, _)| *open != id);
    }

    fn close_all(&mut self) {
        for (_, wire) in &mut self.open {
            wire.close();
        }
        self.open.clear();
    }

    fn drain_until(
        &mut self,
        model: &mut Model,
        actions: &mut Actions,
        pollers: &Pollers,
        until: Instant,
    ) {
        while let Ok(update) = next_before(&self.updates, until) {
            if let Update::TermEnded(id, _) = update {
                self.open.retain(|(open, _)| *open != id);
            }
            send(model.apply(update), actions, self, pollers);
        }
    }
}

impl Drop for Wires {
    fn drop(&mut self) {
        self.close_all();
    }
}

fn pace(model: &Model) -> (Duration, Duration) {
    if model.terminal_live() {
        (TYPE_DRAIN, TYPE_WAIT)
    } else {
        (DRAIN_LIMIT, KEY_WAIT)
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

struct Console<'a, B: Backend, W: Write, R: FnMut(bool) -> io::Result<()>> {
    terminal: &'a mut Terminal<B>,
    screen: &'a mut Screen<W, R>,
}

struct Workers<'a> {
    pollers: &'a Pollers,
    updates: &'a Receiver<Update>,
    actions: &'a mut Actions,
    wires: &'a mut Wires,
}

fn turns<B: Backend, W: Write, R: FnMut(bool) -> io::Result<()>>(
    console: &mut Console<'_, B, W, R>,
    model: &mut Model,
    workers: &mut Workers<'_>,
    next_key: impl FnMut(Duration) -> Result<Option<KeyEvent>, String>,
) -> Result<i32, String> {
    let result = loop_turns(console, model, workers, next_key);
    workers.wires.close_all();
    result
}

fn loop_turns<B: Backend, W: Write, R: FnMut(bool) -> io::Result<()>>(
    console: &mut Console<'_, B, W, R>,
    model: &mut Model,
    workers: &mut Workers<'_>,
    mut next_key: impl FnMut(Duration) -> Result<Option<KeyEvent>, String>,
) -> Result<i32, String> {
    let Console { terminal, screen } = console;
    let Workers {
        pollers,
        updates,
        actions,
        wires,
    } = workers;
    let mut gone = false;
    send(model.ask_changes(), actions, wires, pollers);
    loop {
        let until = Instant::now() + pace(model).0;
        drain(updates, model, &mut gone, actions, wires, pollers);
        match actions.drain_until_quit(model, wires, pollers, until) {
            Some(End::NotStopped { group }) => {
                return Err(shell::not_stopped(group));
            }
            Some(_) => return Ok(model.exit_code()),
            None => {}
        }
        if let Some(lines) = actions.asking.take() {
            let answer = match screen.ask(&lines, actions.asker.as_ref())? {
                Ok(code) => Ok(code == 0),
                Err(error) => Err(error.to_string()),
            };
            terminal.clear().map_err(failed)?;
            send(model.apply(Update::Asked(answer)), actions, wires, pollers);
            pollers.send(Command::Refresh);
        }
        wires.drain_until(model, actions, pollers, until);
        let size = terminal.size().map_err(failed)?;
        model.tick(Instant::now(), SystemTime::now(), (size.width, size.height));
        let area = Rect::new(0, 0, size.width, size.height);
        settle(model, area);
        let inside = screen::terminal_size(model, area);
        send(model.set_terminal_size(inside), actions, wires, pollers);
        terminal.draw(|frame| draw(frame, model)).map_err(failed)?;
        measure(model, area);
        if let Some(key) = next_key(pace(model).1)? {
            send(handle(model, key), actions, wires, pollers);
            send(model.ask_changes(), actions, wires, pollers);
        }
        if model.quit() {
            return Ok(model.exit_code());
        }
    }
}

pub fn settle(model: &mut Model, area: Rect) {
    let measure = screen::run_view(model, area);
    model.set_run_view(measure);
}

pub fn measure(model: &mut Model, area: Rect) {
    model.set_help_overflow(screen::help_overflow(model, area));
    model.set_overview_overflow(screen::overview_overflow(model, area));
    model.set_popup_overflow(screen::popup_overflow(model, area));
    model.set_lab_overflow(screen::lab_overflow(model, area));
    model.set_details_overflow(screen::sonda_overflow(model, area));
    model.set_shown(screen::shown_rows(model, area));
}

fn send(commands: Vec<Command>, actions: &mut Actions, wires: &mut Wires, pollers: &Pollers) {
    for command in commands {
        match &command {
            Command::Run {
                program,
                args,
                login,
            } => actions.start(Invocation {
                program,
                args,
                login,
            }),
            Command::Stop => actions.stop(),
            Command::AskSudo(lines) => actions.asking = Some(lines.clone()),
            Command::TermOpen {
                id,
                program,
                args,
                dir,
                size,
                login,
            } => wires.open(
                *id,
                Invocation {
                    program,
                    args,
                    login,
                },
                dir,
                *size,
            ),
            Command::TermWrite { id, bytes } => {
                if let Some(wire) = wires.wire(*id) {
                    wire.write(bytes);
                }
            }
            Command::TermResize { id, size } => {
                if let Some(wire) = wires.wire(*id) {
                    wire.resize(*size);
                }
            }
            Command::TermClose(id) => wires.close(*id),
            _ => {}
        }
        pollers.send(command);
    }
}

fn drain(
    updates: &Receiver<Update>,
    model: &mut Model,
    gone: &mut bool,
    actions: &mut Actions,
    wires: &mut Wires,
    pollers: &Pollers,
) {
    let until = Instant::now() + pace(model).0;
    while !*gone {
        match next_before(updates, until) {
            Ok(update) => send(model.apply(update), actions, wires, pollers),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                *gone = true;
                send(
                    model.apply(Update::Stopped("poll".to_string())),
                    actions,
                    wires,
                    pollers,
                );
            }
        }
    }
}

fn next_before(updates: &Receiver<Update>, until: Instant) -> Result<Update, TryRecvError> {
    if Instant::now() < until {
        updates.try_recv()
    } else {
        Err(TryRecvError::Empty)
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

fn leave(out: &mut impl Write, raw: &mut impl FnMut(bool) -> io::Result<()>) -> io::Result<()> {
    let left = execute!(out, LeaveAlternateScreen, cursor::Show);
    raw(false).and(left)
}

fn restore(out: &mut impl Write, raw: &mut impl FnMut(bool) -> io::Result<()>) {
    let _ = leave(out, raw);
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

    fn ask(&mut self, lines: &[String], asker: &dyn Asker) -> Result<io::Result<i32>, String> {
        ask_aside(&mut self.out, &mut self.raw, lines, asker).map_err(failed)
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
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::Mutex;

    use std::{mem, ptr};

    use crossterm::event::{KeyCode, KeyModifiers};
    use nobs_core::compose::Compose;
    use nobs_core::containerlab::sudo_refused_line;
    use nobs_core::device::Password;
    use nobs_core::lab;
    use nobs_core::locate::{locate, Kind, Located};
    use nobs_core::project::Rendered;
    use nobs_core::runner::Output;
    use nobs_core::sot::{self, Device, Maintenance, SourceOfTruth};
    use nobs_core::status::{Service, Urls, SERVICES};
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;

    use super::*;
    use crate::aside::tests::{is_child, Child};
    use crate::book::NodeRow;
    use crate::data::stub::Stub;
    use crate::pty::tests::{Call, FakeTerminals};
    use crate::run::tests::FakeLauncher;
    use crate::shell::tests::{defined, every_service_up, finding, listing, SECRET, USER};
    use crate::shell::{Login, RunState};

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
            shows_sonda: false,
        }
    }

    fn composed(env: &[(&str, &str)]) -> Stack {
        Stack {
            compose: fixture("workshop-one/docker-compose.yml"),
            ..stack(env)
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
            nodes: lab.as_ref().and_then(Located::nodes),
            topology: lab.as_ref().map(Located::topology),
            lab,
            stacks,
            sonda: target(),
            home: Workbench::Services,
            chooser,
            source: None,
            source_error: None,
            source_service: None,
            urls: Urls::defaults(),
            program: PathBuf::from("nobs"),
            login: finding(Some(USER), Some(SECRET)),
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
        Actions::new(Arc::new(FakeLauncher::new()), asker(1))
    }

    struct FakeAsker {
        code: i32,
        asked: AtomicUsize,
    }

    impl Asker for FakeAsker {
        fn ask(&self) -> io::Result<i32> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            Ok(self.code)
        }
    }

    fn asker(code: i32) -> Arc<FakeAsker> {
        Arc::new(FakeAsker {
            code,
            asked: AtomicUsize::new(0),
        })
    }

    fn bare_screen() -> Screen<Vec<u8>, impl FnMut(bool) -> io::Result<()>> {
        Screen {
            out: Vec::new(),
            raw: |_| Ok(()),
        }
    }

    fn wires() -> Wires {
        Wires::new(Arc::new(FakeTerminals::default()))
    }

    fn ctrl(typed: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(typed), KeyModifiers::CONTROL)
    }

    fn opens(calls: &[Call]) -> Vec<(u64, (u16, u16))> {
        calls
            .iter()
            .filter_map(|call| match call {
                Call::Open { id, size, .. } => Some((*id, *size)),
                _ => None,
            })
            .collect()
    }

    fn closes(calls: &[Call]) -> Vec<u64> {
        calls
            .iter()
            .filter_map(|call| match call {
                Call::Close(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    fn terminal_turns(
        model: &mut Model,
        wires: &mut Wires,
        (width, height): (u16, u16),
        keys: &[KeyEvent],
    ) -> Result<i32, String> {
        let (pollers, updates) = launch(&scripted(Vec::new()), model);
        let mut keys = keys.iter().copied();
        turns(
            &mut Console {
                terminal: &mut terminal(width, height),
                screen: &mut bare_screen(),
            },
            model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires,
            },
            |_| keys.next().map(Some).ok_or_else(|| PAUSED.to_string()),
        )
    }

    #[test]
    fn the_first_size_is_the_inside_of_the_pane() {
        for (size, inside) in [
            ((60, 15), (8, 58)),
            ((100, 30), (23, 98)),
            ((80, 20), (13, 78)),
        ] {
            let fake = Arc::new(FakeTerminals::default());
            let mut model = devices_in("book-lab");

            terminal_turns(&mut model, &mut Wires::new(fake.clone()), size, &[ch('t')])
                .unwrap_err();

            assert_eq!(opens(&fake.calls()), [(1, inside)], "{size:?}");
        }
    }

    #[test]
    fn a_pane_of_no_height_gives_2_by_2_and_output_that_wraps_there_does_not_crash() {
        let mut model = devices_in("book-lab");
        handle(&mut model, ch('t'));

        let size = screen::terminal_size(&model, Rect::new(0, 0, 0, 0));
        let resized = model.set_terminal_size(size);
        for bytes in ["ab", "cde\r\nfgh", "界界界"] {
            model.apply(Update::TermOutput(1, bytes.as_bytes().to_vec()));
        }

        assert_eq!(size, (2, 2));
        assert_eq!(resized, [Command::TermResize { id: 1, size }]);
        assert_eq!(
            screen::terminal_size(&model, Rect::new(0, 0, 60, 4)),
            (2, 58)
        );
    }

    #[test]
    fn a_new_window_size_reaches_the_session_through_the_loop() {
        let fake = Arc::new(FakeTerminals::default());
        let mut wires = Wires::new(fake.clone());
        let mut model = devices_in("book-lab");
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        for (size, keys) in [((100, 30), vec![ch('t')]), ((80, 20), Vec::new())] {
            let mut keys = keys.into_iter();
            loop_turns(
                &mut Console {
                    terminal: &mut terminal(size.0, size.1),
                    screen: &mut bare_screen(),
                },
                &mut model,
                &mut Workers {
                    pollers: &pollers,
                    updates: &updates,
                    actions: &mut actions(),
                    wires: &mut wires,
                },
                |_| keys.next().map(Some).ok_or_else(|| PAUSED.to_string()),
            )
            .unwrap_err();
        }

        assert_eq!(opens(&fake.calls()), [(1, (23, 98))]);
        assert!(
            fake.calls().contains(&Call::Resize(1, (13, 78))),
            "{:?}",
            fake.calls()
        );
    }

    #[test]
    fn every_open_wire_is_closed_once_when_the_loop_returns_and_on_an_error() {
        let ways: [&[KeyEvent]; 2] = [&[ch('t')], &[ch('t'), ctrl(']'), q(), ch('y')]];
        for keys in ways {
            let fake = Arc::new(FakeTerminals::default());
            let mut wires = Wires::new(fake.clone());
            let mut model = devices_in("book-lab");

            let result = terminal_turns(&mut model, &mut wires, (60, 15), keys);

            assert_eq!(result.is_ok(), keys.len() > 1, "{result:?}");
            assert_eq!(closes(&fake.calls()), [1], "{keys:?}");
            assert!(wires.open.is_empty());
        }
    }

    #[test]
    fn a_holder_that_is_dropped_closes_every_wire() {
        let fake = Arc::new(FakeTerminals::default());
        let mut wires = Wires::new(fake.clone());
        for id in [1, 2] {
            wires.open(
                id,
                Invocation {
                    program: Path::new("nobs"),
                    args: &[],
                    login: &Login::default(),
                },
                Path::new("/"),
                (8, 58),
            );
        }

        drop(wires);

        assert_eq!(closes(&fake.calls()), [1, 2]);
    }

    #[test]
    fn term_close_closes_only_that_wire_and_drops_it_from_the_holder() {
        let fake = Arc::new(FakeTerminals::default());
        let mut wires = Wires::new(fake.clone());
        for id in [1, 2] {
            wires.open(
                id,
                Invocation {
                    program: Path::new("nobs-t-shell"),
                    args: &[],
                    login: &Login::default(),
                },
                Path::new("/"),
                (8, 58),
            );
        }
        let model = devices_in("book-lab");
        let (pollers, _updates) = launch(&scripted(Vec::new()), &model);

        send(
            vec![Command::TermClose(1)],
            &mut actions(),
            &mut wires,
            &pollers,
        );

        assert_eq!(closes(&fake.calls()), [1]);
        let held: Vec<u64> = wires.open.iter().map(|(id, _)| *id).collect();
        assert_eq!(held, [2]);
    }

    fn pids(wires: &Wires, count: usize) -> Vec<libc::pid_t> {
        let mut seen = std::collections::BTreeMap::<u64, String>::new();
        let found = |seen: &std::collections::BTreeMap<u64, String>| -> Vec<libc::pid_t> {
            seen.values()
                .filter_map(|text| {
                    text.split("pid=")
                        .nth(1)?
                        .split_whitespace()
                        .next()?
                        .parse()
                        .ok()
                })
                .collect()
        };
        while found(&seen).len() < count {
            match wires.updates.recv_timeout(Duration::from_secs(5)) {
                Ok(Update::TermOutput(id, bytes)) => seen
                    .entry(id)
                    .or_default()
                    .push_str(&String::from_utf8_lossy(&bytes)),
                other => panic!("{other:?} after {seen:?}"),
            }
        }
        found(&seen)
    }

    fn open_sleeper(wires: &mut Wires, id: u64) {
        wires.open(
            id,
            Invocation {
                program: Path::new("/bin/sh"),
                args: &args(&["-c", "echo pid=$$; exec sleep 10"]),
                login: &Login::default(),
            },
            Path::new(env!("CARGO_MANIFEST_DIR")),
            (24, 80),
        );
    }

    fn gone(pid: libc::pid_t) -> bool {
        let killed = unsafe { libc::kill(pid, 0) };
        killed == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }

    #[test]
    fn real_shells_in_the_holder_are_gone_after_close_all_and_after_a_panic() {
        let mut wires = Wires::new(Arc::new(Ptys));
        open_sleeper(&mut wires, 1);
        open_sleeper(&mut wires, 2);
        let two = pids(&wires, 2);

        wires.close_all();

        let (sender, pid) = mpsc::channel();
        let panicked = thread::spawn(move || {
            let mut wires = Wires::new(Arc::new(Ptys));
            open_sleeper(&mut wires, 3);
            sender.send(pids(&wires, 1)[0]).unwrap();
            panic!("nobs-t-boom");
        })
        .join();
        let third = pid.recv().unwrap();

        assert_eq!(two.len(), 2);
        assert!(two.iter().all(|pid| gone(*pid)), "{two:?}");
        assert!(panicked.is_err());
        assert!(gone(third), "{third}");
    }

    #[test]
    fn the_cursor_of_the_device_shows_only_while_typing() {
        let mut model = devices_in("book-lab");
        model.tick(Instant::now(), SystemTime::now(), (60, 15));
        handle(&mut model, ch('t'));
        model.apply(Update::TermOutput(1, b"nobs-t-x> ".to_vec()));
        let mut cursors = Vec::new();

        for pressed in [None, Some(ctrl(']'))] {
            if let Some(pressed) = pressed {
                handle(&mut model, pressed);
            }
            let mut terminal = terminal(60, 15);
            terminal.draw(|frame| draw(frame, &model)).unwrap();
            cursors.push(terminal.get_cursor_position().unwrap());
        }

        assert_eq!(cursors, [Position::new(11, 4), Position::new(0, 0)]);
    }

    #[test]
    fn the_pace_is_fast_only_while_a_live_session_is_shown() {
        let slow = (DRAIN_LIMIT, KEY_WAIT);
        let fast = (Duration::from_millis(10), Duration::from_millis(20));
        let mut model = devices_in("book-lab");
        let mut seen = vec![pace(&model)];
        for pressed in [ch('t'), ctrl(']'), ch('['), ch(']'), ch('c')] {
            handle(&mut model, pressed);
            seen.push(pace(&model));
        }

        assert_eq!(seen, [slow, fast, fast, slow, fast, slow]);
    }

    #[test]
    fn each_turn_waits_20_ms_while_the_terminal_is_live() {
        let mut model = devices_in("book-lab");
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        let mut waits = Vec::new();

        turns(
            &mut Console {
                terminal: &mut terminal(60, 15),
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
            |wait| {
                waits.push(wait);
                match waits.len() {
                    1 => Ok(Some(ch('t'))),
                    2 => Ok(None),
                    _ => Err(PAUSED.to_string()),
                }
            },
        )
        .unwrap_err();

        assert_eq!(
            waits,
            [100, 20, 20].map(Duration::from_millis),
            "{:?}",
            model.tab()
        );
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
            Update::Services(0, _, result) => Some(result),
            _ => None,
        })
    }

    #[test]
    fn each_turn_waits_100_ms_for_a_key() {
        let mut model = Model::new(start(vec![stack(&[CLOSED])], false));
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        let mut waits = Vec::new();

        turns(
            &mut Console {
                terminal: &mut terminal(80, 24),
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
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
            &mut Console {
                terminal: &mut terminal,
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
            |_| {
                turn += 1;
                if turn == 1 {
                    sender.send(listing(0, lab::services(TWO))).unwrap();
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
        let mut model = Model::new(start(vec![composed(&env)], false));
        model.tick(Instant::now(), SystemTime::now(), (100, 30));
        model.apply(listing(0, lab::services(TWO)));
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
            &mut Console {
                terminal: &mut terminal,
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
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

    fn all_at(stub: &Stub) -> Urls {
        at(stub.url(), Vec::new())
    }

    #[test]
    fn the_device_sources_that_hold_on_never_delay_a_key() {
        let stub = Stub::start_silent(Duration::from_secs(2));
        let begin = Start {
            home: Workbench::Devices,
            source: Some(Arc::new(Holding)),
            urls: all_at(&stub),
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
            key(KeyCode::Char(']')),
        ]
        .into_iter();
        let started = Instant::now();
        let (pollers, updates) = Pollers::start(
            scripted(Vec::new()),
            model.stacks().to_vec(),
            model.selected_stack(),
            target(),
            source.map(|source| (source, None)),
            urls,
            None,
        );

        let result = turns(
            &mut Console {
                terminal: &mut terminal,
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
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
    fn a_device_list_that_hides_the_cursor_row_under_a_kept_filter_tells_the_pollers_the_new_row() {
        let stub = Stub::start(&[]);
        let mut model = devices_model();
        model.apply(Update::Devices(Ok(vec![device("srl1"), device("srl2")])));
        for pressed in [ch('/'), ch('2'), key(KeyCode::Enter)] {
            handle(&mut model, pressed);
        }
        let (pollers, _polled) = Pollers::start(
            scripted(Vec::new()),
            model.stacks().to_vec(),
            model.selected_stack(),
            target(),
            Some((Arc::new(Holding), None)),
            Some(all_at(&stub)),
            None,
        );
        pollers.send(Command::Ask(Service::Logs, true));
        pollers.send(Command::OpenTab(Tab::Logs));
        let (sender, updates) = mpsc::channel();
        sender
            .send(Update::Devices(Ok(vec![
                device("srl0"),
                device("srl1"),
                device("srl2"),
            ])))
            .unwrap();

        drain(
            &updates,
            &mut model,
            &mut false,
            &mut actions(),
            &mut wires(),
            &pollers,
        );

        let asked = || {
            stub.requests()
                .iter()
                .any(|seen| seen.path.contains("device%3D%22srl2%22"))
        };
        let end = Instant::now() + Duration::from_secs(5);
        while !asked() && Instant::now() < end {
            thread::sleep(Duration::from_millis(20));
        }
        pollers.send(Command::Quit);
        assert!(asked(), "{:?}", stub.requests());
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
        let mut model = Model::new(start(vec![composed(&[CLOSED])], false));
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
            drain(
                &updates,
                &mut model,
                &mut gone,
                &mut actions(),
                &mut wires(),
                &pollers,
            );
        }

        turns(
            &mut Console {
                terminal: &mut terminal,
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
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
        model.apply(listing(0, lab::services(LOKI)));
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
            answer(0, "grafana\nloki\n", ""),
            answer(0, TWO, ""),
            answer(0, "grafana\nloki\n", ""),
            answer(1, "", &format!("{DOCKER_DOWN}\n")),
        ]);
        let model = Model::new(start(vec![composed(&[CLOSED])], false));
        let (pollers, updates) = launch(&runner, &model);
        let compose = fixture("workshop-one/docker-compose.yml");
        let compose = compose.to_str().unwrap();

        let rows = services(&updates).unwrap();
        pollers.send(Command::Refresh);
        let failure = services(&updates);

        let names: Vec<(&str, &str)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.state.as_str()))
            .collect();
        assert_eq!(names, [("grafana", "running"), ("loki", "exited")]);
        assert_eq!(failure, Err(DOCKER_DOWN.to_string()));
        let config = (
            "docker".to_string(),
            args(&[
                "compose",
                "--project-name",
                "p",
                "-f",
                compose,
                "config",
                "--services",
            ]),
        );
        let asked = (
            "docker".to_string(),
            args(&[
                "compose",
                "--project-name",
                "p",
                "-f",
                compose,
                "ps",
                "--all",
                "--format",
                "json",
            ]),
        );
        let expected = |index: usize| {
            if index.is_multiple_of(2) {
                &config
            } else {
                &asked
            }
        };
        assert!(runner
            .calls()
            .iter()
            .enumerate()
            .all(|(index, call)| call == expected(index)));
    }

    #[test]
    fn config_answers_become_the_sorted_defined_names_or_the_last_line_of_the_error() {
        let runner = scripted(vec![
            answer(0, "loki\ngrafana\n", ""),
            answer(0, TWO, ""),
            answer(1, "", &format!("{DOCKER_DOWN}\n")),
            answer(0, TWO, ""),
        ]);
        let model = Model::new(start(vec![composed(&[CLOSED])], false));
        let (pollers, updates) = launch(&runner, &model);
        let defined = |updates: &Receiver<Update>| {
            next(updates, Duration::from_secs(5), |update| match update {
                Update::Services(0, defined, _) => Some(defined),
                _ => None,
            })
        };

        let names = defined(&updates);
        pollers.send(Command::Refresh);
        let failure = defined(&updates);

        assert_eq!(names, Ok(vec!["grafana".to_string(), "loki".to_string()]));
        assert_eq!(failure, Err(DOCKER_DOWN.to_string()));
    }

    #[test]
    fn health_shows_ok_and_the_http_code_and_down_for_the_rest() {
        let stub = Stub::start(&[("GET", "/-/ready", 200, ""), ("GET", "/ready", 503, "")]);
        let url = stub.url();
        let env = [CLOSED, ("PROMETHEUS_URL", url), ("LOKI_URL", url)];
        let model = Model::new(start(vec![stack(&env)], false));
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        for service in [Service::Grafana, Service::Metrics, Service::Logs] {
            pollers.send(Command::Ask(service, true));
        }

        let health = next(&updates, Duration::from_secs(10), |update| match update {
            Update::Health(0, list) if list.len() == 3 => Some(list),
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
            vec![composed(&[CLOSED])],
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
                &mut Console {
                    terminal: &mut terminal(80, 24),
                    screen: &mut bare_screen(),
                },
                &mut model,
                &mut Workers {
                    pollers: &pollers,
                    updates: &updates,
                    actions: &mut actions(),
                    wires: &mut wires(),
                },
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
        pollers.send(Command::Ask(Service::Sonda, true));

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
            &mut Console {
                terminal: &mut terminal,
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
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
            &mut Console {
                terminal: &mut terminal(80, 24),
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
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

    fn login_keys(first: char) -> Vec<KeyEvent> {
        [ch(first)]
            .into_iter()
            .chain(USER.chars().map(ch))
            .chain([key(KeyCode::Enter)])
            .chain(SECRET.chars().map(ch))
            .chain([key(KeyCode::Enter)])
            .collect()
    }

    fn acting(launcher: &Arc<FakeLauncher>) -> (Model, Actions) {
        let model = Model::new(start(vec![stack(&[CLOSED])], false));
        (model, Actions::new(launcher.clone(), asker(1)))
    }

    fn press(model: &mut Model, actions: &mut Actions, keys: &[KeyEvent]) -> Result<i32, String> {
        let (pollers, updates) = launch(&scripted(Vec::new()), model);
        let mut keys = keys.iter().copied();
        turns(
            &mut Console {
                terminal: &mut terminal(80, 24),
                screen: &mut bare_screen(),
            },
            model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions,
                wires: &mut wires(),
            },
            |_| keys.next().map(Some).ok_or_else(|| PAUSED.to_string()),
        )
    }

    fn run_state(model: &Model) -> RunState {
        model.run().unwrap().state
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures")
            .join(name)
    }

    fn devices_model() -> Model {
        devices_in("workshop-one")
    }

    fn devices_in(folder: &str) -> Model {
        devices_finding(folder, finding(Some(USER), Some(SECRET)))
    }

    fn devices_finding(folder: &str, login: shell::Lookup) -> Model {
        let dir = fixture(folder);
        let mut model = Model::new(Start {
            nodes: locate(&dir, Some(&dir)).unwrap().nodes(),
            topology: Some(locate(&dir, Some(&dir)).unwrap().topology()),
            lab: Some(locate(&dir, Some(&dir)).unwrap()),
            source: Some(Arc::new(Holding)),
            home: Workbench::Devices,
            login,
            urls: Urls {
                set: SERVICES.iter().map(|row| row.service).collect(),
                ..Urls::defaults()
            },
            ..start(vec![stack(&[CLOSED])], false)
        });
        model.apply(every_service_up(0));
        model.apply(Update::Devices(Ok(vec![device("srl1")])));
        model
    }

    fn lab_model() -> Model {
        let dir = fixture("book-lab");
        let located = locate(&dir, Some(&dir)).unwrap();
        let chapters = shell::stacks(Some(&located), Vec::new()).unwrap();
        Model::new(Start {
            nodes: located.nodes(),
            topology: Some(located.topology()),
            lab: Some(located),
            home: Workbench::Lab,
            ..start(chapters, false)
        })
    }

    #[test]
    fn y_after_an_action_key_starts_the_command_the_model_built() {
        let one = fixture("workshop-one").to_string_lossy().into_owned();
        let book = fixture("book-lab").to_string_lossy().into_owned();
        let workshop = || Model::new(start(vec![stack(&[CLOSED])], false));
        let cases: [(Model, char, Vec<&str>); 5] = [
            (workshop(), 'u', vec!["--dir", "/ws", "ws", "up"]),
            (workshop(), 'd', vec!["--dir", "/ws", "ws", "down"]),
            (
                devices_model(),
                'm',
                vec![
                    "--dir",
                    &one,
                    "nobs-t-one",
                    "maintenance",
                    "-d",
                    "srl1",
                    "--state",
                ],
            ),
            (
                lab_model(),
                'D',
                vec![
                    "--dir",
                    &book,
                    "lab",
                    "deploy",
                    "--sudo-no-prompt",
                    "-s",
                    "ch1",
                ],
            ),
            (
                lab_model(),
                'X',
                vec![
                    "--dir",
                    &book,
                    "lab",
                    "destroy",
                    "--sudo-no-prompt",
                    "-s",
                    "ch1",
                ],
            ),
        ];

        let mut seen = Vec::new();
        let mut wanted = Vec::new();

        for (mut model, typed, words) in cases {
            let launcher = Arc::new(FakeLauncher::new());
            let mut actions = Actions::new(launcher.clone(), asker(1));
            let result = press(&mut model, &mut actions, &[ch(typed), ch('y')]);
            seen.push((typed, result, launcher.calls()));
            wanted.push((
                typed,
                Err(PAUSED.to_string()),
                vec![(PathBuf::from("nobs"), args(&words))],
            ));
        }

        assert_eq!(seen, wanted);
    }

    fn project_start() -> Start {
        let dir = fixture("project-two-node");
        let located = locate(&dir, Some(&dir)).unwrap();
        let chapters = shell::stacks(Some(&located), Vec::new()).unwrap();
        Start {
            nodes: located.nodes(),
            topology: Some(located.topology()),
            lab: Some(located),
            home: Workbench::Lab,
            ..start(chapters, false)
        }
    }

    #[test]
    fn y_after_x_in_a_project_folder_starts_containerlab_destroy_with_the_sudo_flag() {
        let dir = fixture("project-two-node").to_string_lossy().into_owned();
        let launcher = Arc::new(FakeLauncher::new());
        let mut actions = Actions::new(launcher.clone(), asker(1));
        let mut model = Model::new(project_start());

        press(&mut model, &mut actions, &[ch('X'), ch('y')]).unwrap_err();

        let words = ["--dir", &dir, "containerlab", "destroy", "--sudo-no-prompt"];
        assert_eq!(launcher.calls(), [(PathBuf::from("nobs"), args(&words))]);
    }

    #[test]
    fn the_loop_starts_the_files_poller_of_a_project_folder_and_the_model_gets_its_answer() {
        let (mut model, pollers, updates) = started(project_start(), scripted(Vec::new()));
        let mut turns_left = 40;

        turns(
            &mut Console {
                terminal: &mut terminal(80, 24),
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
            |_| {
                thread::sleep(Duration::from_millis(50));
                turns_left -= 1;
                if turns_left == 0 {
                    Err(PAUSED.to_string())
                } else {
                    Ok(None)
                }
            },
        )
        .unwrap_err();

        assert_eq!(model.files(), Some(&Rendered::NotRendered));
    }

    #[test]
    fn a_typed_login_reaches_the_launcher_and_none_of_its_arguments() {
        let launcher = Arc::new(FakeLauncher::new());
        let mut actions = Actions::new(launcher.clone(), asker(1));
        let mut model = devices_finding("book-lab", finding(None, None));
        let keys = login_keys('g');

        press(&mut model, &mut actions, &keys).unwrap_err();

        let typed = Login {
            user: Some(USER.to_string()),
            password: Some(Password::new(SECRET.to_string())),
        };
        assert_eq!(launcher.logins(), [typed]);
        let calls = launcher.calls();
        assert_eq!(calls.len(), 1);
        assert!(!format!("{calls:?}").contains(SECRET), "{calls:?}");
    }

    #[test]
    fn t_with_a_typed_login_opens_the_terminal_the_model_built_and_no_argument_holds_the_password()
    {
        let fake = Arc::new(FakeTerminals::default());
        let mut model = devices_finding("book-lab", finding(None, None));
        let book = fixture("book-lab");
        let keys = login_keys('t');

        terminal_turns(&mut model, &mut Wires::new(fake.clone()), (80, 20), &keys).unwrap_err();

        let calls = fake.calls();
        let Some(Call::Open { args: given, .. }) = calls.first() else {
            panic!("{calls:?}");
        };
        assert!(!format!("{given:?}").contains(SECRET), "{given:?}");
        assert_eq!(
            calls.first(),
            Some(&Call::Open {
                id: 1,
                program: PathBuf::from("nobs"),
                args: [OsString::from("--dir"), book.clone().into_os_string()]
                    .into_iter()
                    .chain(args(&["device", "shell", "srl1"]))
                    .collect(),
                dir: book,
                size: (13, 78),
                login: Login {
                    user: Some(USER.to_string()),
                    password: Some(Password::new(SECRET.to_string())),
                },
            })
        );
    }

    #[test]
    fn the_end_of_an_action_asks_the_pollers_to_refresh() {
        let runner = scripted(Vec::new());
        let (pollers, updates) = Pollers::start(
            runner,
            Vec::new(),
            0,
            target(),
            None,
            None,
            Some(LabFiles {
                topology: fixture("book-lab/containerlab/lab.yml"),
                project: None,
            }),
        );
        let nodes = |limit: Duration| {
            next(&updates, limit, |update| {
                matches!(update, Update::Nodes(_)).then_some(())
            })
        };
        nodes(Duration::from_secs(5));
        let launcher = Arc::new(FakeLauncher::new());
        let (mut model, mut actions) = acting(&launcher);
        press(&mut model, &mut actions, &[ch('u'), ch('y')]).unwrap_err();
        launcher
            .sender()
            .unwrap()
            .send(Update::ActionEnded(End::Exited(0)))
            .unwrap();
        let (_alive, quiet) = mpsc::channel();

        turns(
            &mut Console {
                terminal: &mut terminal(80, 24),
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &quiet,
                actions: &mut actions,
                wires: &mut wires(),
            },
            |_| Err(PAUSED.to_string()),
        )
        .unwrap_err();

        nodes(Duration::from_secs(2));
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
    fn ctrl_c_stops_a_get_and_a_later_get_fills_the_tab() {
        let config = "set / acl acl-filter cpm type ipv4 statistics-per-entry true";
        let launcher = Arc::new(FakeLauncher::new());
        let mut actions = Actions::new(launcher.clone(), asker(1));
        let mut model = devices_in("book-lab");
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        press(&mut model, &mut actions, &[ch('g')]).unwrap_err();
        let sender = launcher.sender().unwrap();
        sender.send(Update::ActionLine(config.to_string())).unwrap();
        press(&mut model, &mut actions, &[ctrl_c]).unwrap_err();

        assert_eq!(launcher.stops(), 1);
        assert_eq!(run_state(&model), RunState::Ended(End::Stopped));
        assert_eq!(model.config_of("srl1"), None);

        press(&mut model, &mut actions, &[ch('g')]).unwrap_err();
        let sender = launcher.sender().unwrap();
        sender.send(Update::ActionLine(config.to_string())).unwrap();
        sender.send(Update::ActionEnded(End::Exited(0))).unwrap();
        press(&mut model, &mut actions, &[]).unwrap_err();

        assert_eq!(launcher.calls().len(), 2);
        assert_eq!(model.config_of("srl1").unwrap().lines, [config]);
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
            &mut Console {
                terminal: &mut terminal(80, 24),
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions,
                wires: &mut wires(),
            },
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
            &mut Console {
                terminal: &mut terminal(80, 24),
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions,
                wires: &mut wires(),
            },
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

    fn refused_destroy(asker: Arc<dyn Asker>) -> (Model, Actions, Arc<FakeLauncher>) {
        let launcher = Arc::new(FakeLauncher::new());
        let mut model = lab_model();
        let mut actions = Actions::new(launcher.clone(), asker);
        press(&mut model, &mut actions, &[ch('X'), ch('y')]).unwrap_err();
        let sender = launcher.sender().unwrap();
        for line in [
            "sudo: a password is required".to_string(),
            sudo_refused_line("lab destroy"),
        ] {
            sender.send(Update::ActionError(line)).unwrap();
        }
        sender.send(Update::ActionEnded(End::Exited(1))).unwrap();
        (model, actions, launcher)
    }

    fn position(bytes: &[u8], part: &[u8]) -> usize {
        bytes
            .windows(part.len())
            .position(|window| window == part)
            .unwrap_or_else(|| panic!("{:?}", String::from_utf8_lossy(part)))
    }

    fn handlers() -> Vec<libc::sighandler_t> {
        [libc::SIGINT, libc::SIGQUIT]
            .map(|signal| {
                let mut current: libc::sigaction = unsafe { mem::zeroed() };
                unsafe { libc::sigaction(signal, ptr::null(), &mut current) };
                current.sa_sigaction
            })
            .to_vec()
    }

    fn passed_in_a_terminal(test: &str) {
        let (seen, code) = Child::start(test).end();
        assert_eq!(code, 0, "{seen}");
        assert!(seen.contains("1 passed"), "{seen}");
    }

    #[test]
    fn a_sudo_refusal_steps_aside_once_and_the_answer_reaches_the_model_before_the_next_frame() {
        if !is_child() {
            return passed_in_a_terminal(
                "tests::a_sudo_refusal_steps_aside_once_and_the_answer_reaches_the_model_before_the_next_frame",
            );
        }
        let fake = asker(0);
        let (mut model, mut actions, launcher) = refused_destroy(fake.clone());
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        let mut terminal = terminal(80, 24);
        let mut raw = Vec::new();
        let mut screen = Screen {
            out: Vec::new(),
            raw: |on| {
                raw.push(on);
                Ok(())
            },
        };
        let mut frames = Vec::new();
        let before = handlers();

        for _ in 0..2 {
            turns(
                &mut Console {
                    terminal: &mut terminal,
                    screen: &mut screen,
                },
                &mut model,
                &mut Workers {
                    pollers: &pollers,
                    updates: &updates,
                    actions: &mut actions,
                    wires: &mut wires(),
                },
                |_| Err(PAUSED.to_string()),
            )
            .unwrap_err();
            frames.push((
                fake.asked.load(Ordering::SeqCst),
                launcher.calls().len(),
                text(&terminal).contains("password is required"),
            ));
        }
        let after = handlers();
        let out = mem::take(&mut screen.out);
        drop(screen);

        assert_eq!(frames, [(1, 2, false), (1, 2, false)]);
        assert_eq!(after, before);
        assert_eq!(raw, [false, true, false]);
        let (mut leave, mut show, mut enter) = (Vec::new(), Vec::new(), Vec::new());
        execute!(leave, LeaveAlternateScreen).unwrap();
        execute!(show, cursor::Show).unwrap();
        execute!(enter, EnterAlternateScreen).unwrap();
        let first = position(&out, b"nobs: lab destroy runs containerlab with sudo;");
        let second = position(
            &out,
            b"      sudo asks for your password, nobs never sees it",
        );
        let shown = position(&out, &show);
        assert!(position(&out, &leave) < shown && shown < first, "{out:?}");
        assert!(first < second && second < position(&out, &enter), "{out:?}");
    }

    #[test]
    fn a_sudo_that_ends_with_any_code_but_0_shows_not_run_and_starts_nothing() {
        if !is_child() {
            return passed_in_a_terminal(
                "tests::a_sudo_that_ends_with_any_code_but_0_shows_not_run_and_starts_nothing",
            );
        }
        for code in [1, 130] {
            let (mut model, mut actions, launcher) = refused_destroy(asker(code));
            let (pollers, updates) = launch(&scripted(Vec::new()), &model);

            turns(
                &mut Console {
                    terminal: &mut terminal(80, 24),
                    screen: &mut bare_screen(),
                },
                &mut model,
                &mut Workers {
                    pollers: &pollers,
                    updates: &updates,
                    actions: &mut actions,
                    wires: &mut wires(),
                },
                |_| Err(PAUSED.to_string()),
            )
            .unwrap_err();

            assert_eq!(run_state(&model), RunState::NotRun, "{code}");
            assert_eq!(
                model.run().unwrap().lines,
                ["nobs: lab destroy: sudo got no password; nothing ran"],
                "{code}"
            );
            assert_eq!(launcher.calls().len(), 1, "{code}");
        }
    }

    #[test]
    fn a_raw_mode_that_fails_on_the_way_back_ends_the_screen_and_restores_the_terminal() {
        if !is_child() {
            return passed_in_a_terminal(
                "tests::a_raw_mode_that_fails_on_the_way_back_ends_the_screen_and_restores_the_terminal",
            );
        }
        let (mut model, mut actions, _launcher) = refused_destroy(asker(0));
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        let before = handlers();
        let mut raw = Vec::new();
        let mut screen = Screen {
            out: Vec::new(),
            raw: |on| {
                raw.push(on);
                if on {
                    Err(io::Error::other("nobs-t-no-raw"))
                } else {
                    Ok(())
                }
            },
        };

        let result = turns(
            &mut Console {
                terminal: &mut terminal(80, 24),
                screen: &mut screen,
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions,
                wires: &mut wires(),
            },
            |_| Err(PAUSED.to_string()),
        );
        let after = handlers();
        drop(screen);

        assert_eq!(result, Err("the screen failed: nobs-t-no-raw".to_string()));
        assert_eq!(after, before);
        assert_eq!(raw, [false, true, false]);
    }

    struct Waiting {
        runner: Arc<Scripted>,
        sonda: Stub,
        calls: Mutex<Vec<(usize, usize)>>,
    }

    impl Waiting {
        fn count(&self) -> (usize, usize) {
            (self.runner.calls().len(), self.sonda.requests().len())
        }
    }

    impl Asker for Waiting {
        fn ask(&self) -> io::Result<i32> {
            let before = self.count();
            thread::sleep(poll::PS_PERIOD + Duration::from_millis(500));
            let after = self.count();
            self.calls.lock().unwrap().extend([before, after]);
            Ok(1)
        }
    }

    #[test]
    fn the_pollers_start_no_call_while_the_screen_is_aside_and_start_again_after() {
        if !is_child() {
            return passed_in_a_terminal(
                "tests::the_pollers_start_no_call_while_the_screen_is_aside_and_start_again_after",
            );
        }
        let runner = scripted(Vec::new());
        let waiting = Arc::new(Waiting {
            runner: runner.clone(),
            sonda: Stub::start(&[("GET", "/scenarios", 200, r#"{"scenarios":[]}"#)]),
            calls: Mutex::new(Vec::new()),
        });
        let (mut model, mut actions, _launcher) = refused_destroy(waiting.clone());
        let (pollers, updates) = Pollers::start(
            runner.clone(),
            model.stacks().to_vec(),
            model.selected_stack(),
            SondaTarget {
                server: Some(waiting.sonda.url().to_string()),
                api_key: None,
            },
            None,
            None,
            None,
        );
        pollers.send(Command::Ask(Service::Sonda, true));
        let _ = services(&updates);
        let end = Instant::now() + Duration::from_secs(5);
        while waiting.count().1 == 0 && Instant::now() < end {
            thread::sleep(Duration::from_millis(20));
        }

        turns(
            &mut Console {
                terminal: &mut terminal(80, 24),
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions,
                wires: &mut wires(),
            },
            |_| Err(PAUSED.to_string()),
        )
        .unwrap_err();
        let aside = waiting.calls.lock().unwrap().clone();
        let _ = services(&updates);
        let end = Instant::now() + Duration::from_secs(5);
        while waiting.count().1 == aside[1].1 && Instant::now() < end {
            thread::sleep(Duration::from_millis(20));
        }

        assert!(aside[0].1 > 0, "{aside:?}");
        assert_eq!(aside[0], aside[1]);
        let again = waiting.count();
        assert!(
            again.0 > aside[1].0 && again.1 > aside[1].1,
            "{again:?} {aside:?}"
        );
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
            Some(LabFiles {
                topology,
                project: None,
            }),
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
    fn the_nodes_thread_of_a_topology_not_written_yet_gives_no_nodes_and_no_error() {
        let (_pollers, updates) = Pollers::start(
            scripted(Vec::new()),
            Vec::new(),
            0,
            target(),
            None,
            None,
            Some(LabFiles {
                topology: fixture("project-two-node/containerlab/lab.clab.yml"),
                project: None,
            }),
        );

        let rows = next(&updates, Duration::from_secs(5), |update| match update {
            Update::Nodes(result) => Some(result),
            _ => None,
        });

        assert_eq!(rows, Ok(Vec::new()));
    }

    #[test]
    fn the_ps_of_a_book_lab_chapter_leaves_out_the_other_chapters_and_a_workshop_keeps_them() {
        let runner = scripted(Vec::new());
        let chapter = Stack {
            name: "ch1".to_string(),
            kind: StackKind::BookLab,
            ..composed(&[CLOSED])
        };
        let (_pollers, updates) = Pollers::start(
            runner.clone(),
            vec![composed(&[CLOSED]), chapter],
            0,
            target(),
            None,
            None,
            None,
        );

        next(&updates, Duration::from_secs(5), |update| {
            matches!(update, Update::Services(1, ..)).then_some(())
        });

        let compose = fixture("workshop-one/docker-compose.yml");
        let project_and_file = [
            "compose",
            "--project-name",
            "p",
            "-f",
            compose.to_str().unwrap(),
        ];
        let ps = |extra: &[&str]| {
            let mut words = project_and_file.to_vec();
            words.extend(["ps", "--all"]);
            words.extend(extra);
            words.extend(["--format", "json"]);
            (Compose::PROGRAM.to_string(), args(&words))
        };
        let config = {
            let mut words = project_and_file.to_vec();
            words.extend(["config", "--services"]);
            (Compose::PROGRAM.to_string(), args(&words))
        };
        assert_eq!(
            runner.calls()[..4],
            [config.clone(), ps(&[]), config, ps(&["--orphans=false"])]
        );
    }

    fn help_after(presses: usize, back: bool) -> Vec<String> {
        let mut model = Model::new(start(vec![stack(&[CLOSED])], false));
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        let mut terminal = terminal(60, 15);
        let mut keys = std::iter::once(ch('?'))
            .chain(std::iter::repeat_n(ch('j'), presses))
            .chain(back.then(|| ch('k')));
        turns(
            &mut Console {
                terminal: &mut terminal,
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
            |_| keys.next().map(Some).ok_or_else(|| PAUSED.to_string()),
        )
        .unwrap_err();
        let screen: Vec<char> = text(&terminal).chars().collect();
        screen
            .chunks(60)
            .map(String::from_iter)
            .skip_while(|row| !row.contains("╔ keys"))
            .skip(1)
            .take_while(|row| !row.contains("PgUp PgDn"))
            .collect()
    }

    #[test]
    fn j_scrolls_the_help_to_its_last_entry_and_no_further() {
        let shows = |rows: &[String], pair: [&str; 2]| {
            rows.iter().any(|row| {
                let words: Vec<&str> = row.split_whitespace().collect();
                words.windows(2).any(|seen| seen == pair)
            })
        };
        let opened = help_after(0, false);
        let end = help_after(20, false);
        let back = help_after(20, true);

        assert!(!shows(&opened, ["Ctrl-C", "stop"]), "{opened:?}");
        assert!(shows(&end, ["Ctrl-C", "stop"]), "{end:?}");
        assert_ne!(back, end);
        assert_eq!(back[1..], end[..end.len() - 1]);
    }

    const HELP_SIZES: [(u16, u16); 3] = [(60, 15), (80, 24), (100, 30)];

    fn help_turn(model: &mut Model, size: (u16, u16), keys: &[KeyEvent]) -> Vec<String> {
        let (pollers, updates) = launch(&scripted(Vec::new()), model);
        drawn_turn(model, size, keys, &pollers, &updates)
            .into_iter()
            .map(|row| row.replace('║', " "))
            .collect()
    }

    fn quiet_turn(model: &mut Model, size: (u16, u16), keys: &[KeyEvent]) -> Vec<String> {
        let (pollers, _polled) = launch(&scripted(Vec::new()), model);
        let (_sender, updates) = mpsc::channel();
        drawn_turn(model, size, keys, &pollers, &updates)
    }

    fn drawn_turn(
        model: &mut Model,
        (width, height): (u16, u16),
        keys: &[KeyEvent],
        pollers: &Pollers,
        updates: &Receiver<Update>,
    ) -> Vec<String> {
        let mut terminal = terminal(width, height);
        let mut keys = keys.iter().copied();
        turns(
            &mut Console {
                terminal: &mut terminal,
                screen: &mut bare_screen(),
            },
            model,
            &mut Workers {
                pollers,
                updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
            |_| keys.next().map(Some).ok_or_else(|| PAUSED.to_string()),
        )
        .unwrap_err();
        let screen: Vec<char> = text(&terminal).replace('│', " ").chars().collect();
        screen
            .chunks(usize::from(width))
            .map(String::from_iter)
            .collect()
    }

    fn shows_entry(rows: &[String], (key, meaning): &(String, &str)) -> bool {
        let wanted: Vec<&str> = key
            .split_whitespace()
            .chain(meaning.split_whitespace())
            .collect();
        rows.iter().any(|row| {
            let words: Vec<&str> = row.split_whitespace().collect();
            words.windows(wanted.len()).any(|seen| seen == wanted)
        })
    }

    fn services_help_model() -> Model {
        Model::new(start(vec![stack(&[CLOSED])], false))
    }

    fn limit(model: &Model, (width, height): (u16, u16)) -> usize {
        screen::help_overflow(model, Rect::new(0, 0, width, height))
    }

    #[test]
    fn j_reaches_the_last_help_entry_at_every_size_with_the_pane_open_or_closed() {
        let builds: [fn() -> Model; 3] = [services_help_model, devices_model, lab_model];
        for build in builds {
            for size in HELP_SIZES {
                for pane in [false, true] {
                    let mut model = build();
                    let mut keys: Vec<KeyEvent> = pane.then(|| ch('o')).into_iter().collect();
                    keys.push(ch('?'));
                    keys.extend(std::iter::repeat_n(ch('j'), 20));

                    let rows = help_turn(&mut model, size, &keys);

                    let entries = screen::help_entries(&model);
                    let last = entries.last().unwrap();
                    assert!(
                        shows_entry(&rows, last),
                        "{:?} {size:?} pane {pane}\n{}",
                        model.workbench(),
                        rows.join("\n")
                    );
                }
            }
        }
    }

    #[test]
    fn after_a_shrink_a_grow_and_a_shrink_the_help_scroll_stays_in_its_limit_and_one_k_moves_it() {
        let mut model = services_help_model();
        help_turn(&mut model, (100, 30), &[ch('?')]);
        help_turn(&mut model, (60, 15), &[ch('j'); 20]);
        assert_eq!(model.help_scroll(), limit(&model, (60, 15)));

        for size in [(60, 16), (60, 15)] {
            let before = help_turn(&mut model, size, &[]);
            assert!(
                model.help_scroll() <= limit(&model, size),
                "{size:?}: scroll {} limit {}",
                model.help_scroll(),
                limit(&model, size)
            );
            let after = help_turn(&mut model, size, &[ch('k')]);
            assert_ne!(before, after, "{size:?}\n{}", after.join("\n"));
        }
    }

    fn with_sessions(name: &str, count: usize) -> Device {
        let sessions = (1..=count)
            .map(|index| sot::Session {
                peer: format!("p{index:02}"),
                afi_safi: None,
                expected_state: None,
                remote_as: None,
                expected_prefixes: None,
                reason: None,
            })
            .collect();
        Device {
            sessions,
            ..device(name)
        }
    }

    fn overview_model(sessions: usize) -> Model {
        let mut model = devices_model();
        model.apply(Update::Devices(Ok(vec![
            with_sessions("srl1", sessions),
            with_sessions("srl2", sessions),
        ])));
        model
    }

    #[test]
    fn the_overview_is_back_at_its_first_line_with_a_new_device_and_inside_when_lines_leave() {
        let mut model = overview_model(12);
        let mut keys = vec![key(KeyCode::Tab)];
        keys.extend([ch('j'); 5]);
        help_turn(&mut model, (60, 15), &keys);
        assert_eq!(model.overview_scroll(), 5);
        help_turn(&mut model, (60, 15), &[key(KeyCode::Tab), ch('j')]);
        assert_eq!(model.selected_device().unwrap().name, "srl2");
        assert_eq!(model.overview_scroll(), 0);

        let mut model = overview_model(12);
        let mut keys = vec![key(KeyCode::Tab)];
        keys.extend([ch('j'); 20]);
        help_turn(&mut model, (60, 15), &keys);
        let limit = model.overview_scroll();
        assert_eq!(
            limit,
            screen::overview_overflow(&model, Rect::new(0, 0, 60, 15))
        );
        model.apply(Update::Devices(Ok(vec![with_sessions("srl1", 2)])));
        assert_eq!(model.overview_scroll(), limit);

        let rows = help_turn(&mut model, (60, 15), &[]).join("\n");

        assert!(rows.contains("kind "), "{rows}");
        assert_eq!(model.overview_scroll(), 0);
    }

    fn boxed_model() -> Model {
        let mut model = devices_model();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let text: Vec<String> = (1..=12).map(|index| format!("text {index:02}")).collect();
        model.apply(Update::DeviceLogs(
            "srl1".to_string(),
            Ok(vec![nobs_core::loki::Line {
                nanoseconds: i64::try_from(now).unwrap(),
                text: text.join("\n"),
            }]),
        ));
        model
    }

    fn popup_limit(model: &Model, (width, height): (u16, u16)) -> usize {
        screen::popup_overflow(model, Rect::new(0, 0, width, height))
    }

    #[test]
    fn the_box_opens_at_its_first_line_and_its_first_line_exists_when_the_room_changes() {
        let mut model = boxed_model();
        let mut keys = vec![key(KeyCode::Tab), ch(']'), ch(']'), key(KeyCode::Enter)];
        keys.extend([ch('j'); 5]);
        help_turn(&mut model, (60, 15), &keys);
        assert_eq!(model.tab(), shell::Tab::Timeline);
        assert!(model.popup());
        assert_eq!(model.popup_scroll(), 5);
        handle(&mut model, key(KeyCode::Esc));
        assert!(!model.popup());
        handle(&mut model, key(KeyCode::Enter));
        assert!(model.popup());
        assert_eq!(model.popup_scroll(), 0);

        help_turn(&mut model, (60, 15), &[ch('j'); 20]);
        assert_eq!(model.popup_scroll(), popup_limit(&model, (60, 15)));
        for size in [(100, 30), (60, 15)] {
            let rows = help_turn(&mut model, size, &[]).join("\n");

            let first = model.popup_scroll() + 1;
            assert!(
                rows.contains(&format!("text {first:02}")),
                "{size:?}\n{rows}"
            );
            assert!(
                model.popup_scroll() <= popup_limit(&model, size),
                "{size:?}"
            );
        }
    }

    #[test]
    fn j_on_the_lab_overview_in_the_loop_draws_the_line_the_overview_hid() {
        let mut model = lab_model();
        handle(&mut model, ch('D'));
        handle(&mut model, ch('y'));
        model.apply(Update::ActionLine("nobs: lab deploy: refused".to_string()));
        model.apply(Update::ActionEnded(End::Exited(1)));
        model.apply(Update::Services(
            0,
            defined(&["grafana"]),
            Err("exit 1".to_string()),
        ));
        let size = (60, 15);

        let before = quiet_turn(&mut model, size, &[key(KeyCode::Tab), key(KeyCode::Tab)]);
        let after = quiet_turn(&mut model, size, &[ch('j')]);

        let screens = [before.join("\n"), after.join("\n")];
        assert_eq!(model.focus(), Focus::Lines);
        assert!(screens[0].contains(" ps ch1: exit 1"), "{}", screens[0]);
        assert!(!screens[0].contains("║ last"), "{}", screens[0]);
        assert!(screens[1].contains("║ last"), "{}", screens[1]);
    }

    #[test]
    fn tab_in_the_loop_reaches_the_run_pane_the_loop_measured() {
        let mut model = lab_model();
        handle(&mut model, ch('D'));
        handle(&mut model, ch('y'));
        model.apply(Update::ActionLine("nobs: lab deploy: refused".to_string()));
        model.apply(Update::ActionEnded(End::Exited(1)));

        let rows = quiet_turn(&mut model, (60, 15), &[key(KeyCode::Tab); 3]).join("\n");

        assert_eq!(model.focus(), Focus::Run);
        assert!(rows.contains("╔ run  lab deploy"), "{rows}");
    }

    #[test]
    fn a_line_drained_in_a_turn_shows_in_that_turns_frame_of_a_following_run_pane() {
        let mut model = lab_model();
        handle(&mut model, ch('D'));
        handle(&mut model, ch('y'));
        for index in 1..=30 {
            model.apply(Update::ActionLine(format!("step {index:02}")));
        }
        quiet_turn(&mut model, (60, 15), &[key(KeyCode::Tab); 3]);
        let (pollers, _polled) = launch(&scripted(Vec::new()), &model);
        let (sender, updates) = mpsc::channel();
        sender
            .send(Update::ActionLine("step 31".to_string()))
            .unwrap();

        let rows = drawn_turn(&mut model, (60, 15), &[], &pollers, &updates).join("\n");

        assert_eq!(model.focus(), Focus::Run);
        assert!(rows.contains("step 31"), "{rows}");
    }

    #[test]
    fn j_on_the_sonda_details_in_the_loop_moves_the_text_the_box_hid() {
        let mut model = Model::new(start(vec![stack(&[CLOSED])], false));
        model.apply(every_service_up(0));
        handle(&mut model, key(KeyCode::Tab));
        handle(&mut model, ch('['));
        let fields: Vec<String> = (1..=30)
            .map(|index| format!(r#""f{index:02}":1"#))
            .collect();
        model.apply(Update::Sonda(data::Update {
            at: Instant::now(),
            list: Ok(r#"{"scenarios":[{"id":"id00","name":"n00","state":"running","elapsed_secs":1,"degraded":false}]}"#.to_string()),
            selected: Some((
                "id00".to_string(),
                Ok(r#"{"name":"cpu"}"#.to_string()),
                Ok(format!("{{{}}}", fields.join(","))),
            )),
        }));
        assert_eq!(model.tab(), Tab::Sonda);

        quiet_turn(&mut model, (60, 15), &[key(KeyCode::Tab), ch('j')]);

        assert_eq!(model.focus(), Focus::Lines);
        assert_eq!(model.sonda().details_scroll(), 1);
    }

    #[test]
    fn the_lab_help_covers_the_open_pane_and_j_brings_all_seventeen_entries_into_view_at_60_by_15()
    {
        let mut model = lab_model();

        let rows = help_turn(&mut model, (60, 15), &[ch('o'), ch('?')]);
        let scrolled = help_turn(
            &mut lab_model(),
            (60, 15),
            &[ch('o'), ch('?'), ch('j'), ch('j'), ch('j')],
        );

        let entries = screen::help_entries(&model);
        let screen = format!("{}\n\n{}", rows.join("\n"), scrolled.join("\n"));
        assert_eq!(model.workbench(), Workbench::Lab);
        assert!(model.run_pane());
        assert!(rows[2].contains("╔ keys"), "{screen}");
        assert!(rows[13].contains('╚'), "{screen}");
        assert_eq!(entries.len(), 17);
        for entry in &entries {
            assert!(
                shows_entry(&rows, entry) || shows_entry(&scrolled, entry),
                "{entry:?}\n{screen}"
            );
        }
        assert!(rows.iter().any(|row| row.contains("j/k more")), "{screen}");
        assert!(rows[13].contains("═ ↓3 ═╝"), "{screen}");
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

    struct Counter {
        url: String,
        accepted: Arc<AtomicUsize>,
    }

    impl Counter {
        fn start() -> Counter {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let accepted = Arc::new(AtomicUsize::new(0));
            let seen = Arc::clone(&accepted);
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    seen.fetch_add(1, Ordering::SeqCst);
                    drop(stream);
                }
            });
            Counter { url, accepted }
        }

        fn count(&self) -> usize {
            self.accepted.load(Ordering::SeqCst)
        }
    }

    struct Dialing(String);

    impl SourceOfTruth for Dialing {
        fn devices(&self) -> Result<Vec<Device>, sot::Error> {
            let _ = TcpStream::connect(self.0.trim_start_matches("http://"));
            Ok(Vec::new())
        }

        fn device(&self, _name: &str) -> Result<Option<Device>, sot::Error> {
            Ok(None)
        }

        fn set_maintenance(&self, _device: &str, _state: bool) -> Result<Maintenance, sot::Error> {
            Ok(Maintenance { previous: false })
        }
    }

    struct Docker {
        names: Vec<&'static str>,
        up: AtomicBool,
        sleep: Duration,
    }

    impl Docker {
        fn new(names: &[&'static str], up: bool) -> Arc<Docker> {
            Arc::new(Docker {
                names: names.to_vec(),
                up: AtomicBool::new(up),
                sleep: Duration::ZERO,
            })
        }
    }

    impl Runner for Docker {
        fn run(&self, _program: &str, _args: &[OsString]) -> io::Result<i32> {
            Ok(0)
        }

        fn output(&self, _program: &str, args: &[OsString]) -> io::Result<Output> {
            thread::sleep(self.sleep);
            let config = args.iter().any(|arg| arg == "config");
            let state = if self.up.load(Ordering::SeqCst) {
                "running"
            } else {
                "exited"
            };
            let stdout: String = self
                .names
                .iter()
                .map(|name| match config {
                    true => format!("{name}\n"),
                    false => format!("{{\"Service\":\"{name}\",\"State\":\"{state}\"}}\n"),
                })
                .collect();
            Ok(answer(0, &stdout, ""))
        }
    }

    fn at(url: &str, set: Vec<Service>) -> Urls {
        Urls {
            grafana: url.to_string(),
            prometheus: url.to_string(),
            loki: url.to_string(),
            alertmanager: url.to_string(),
            infrahub: url.to_string(),
            prefect: url.to_string(),
            sonda: url.to_string(),
            set,
        }
    }

    fn every_setting(url: &str) -> Vec<(&'static str, &str)> {
        SERVICES
            .iter()
            .filter_map(|row| row.setting)
            .map(|name| (name, url))
            .collect()
    }

    fn every_service() -> Vec<Service> {
        SERVICES.iter().map(|row| row.service).collect()
    }

    fn compose_names() -> Vec<&'static str> {
        SERVICES.iter().map(|row| row.compose).collect()
    }

    fn closed_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    fn polled(
        begin: Start,
        runner: Arc<dyn Runner + Send + Sync>,
        sonda: Option<&str>,
    ) -> (Model, Pollers, Receiver<Update>) {
        let model = Model::new(begin.clone());
        let (pollers, updates) = Pollers::start(
            runner,
            model.stacks().to_vec(),
            model.selected_stack(),
            SondaTarget {
                server: sonda.map(str::to_string),
                api_key: None,
            },
            begin.source.map(|source| (source, begin.source_service)),
            Some(begin.urls),
            None,
        );
        pollers.send(Command::SelectDevice("srl1".to_string()));
        pollers.send(Command::OpenTab(Tab::Timeline));
        (model, pollers, updates)
    }

    fn turn_until(
        model: &mut Model,
        pollers: &Pollers,
        updates: &Receiver<Update>,
        limit: Duration,
        done: impl Fn(&Model) -> bool,
    ) -> bool {
        let end = Instant::now() + limit;
        let mut actions = actions();
        loop {
            turns(
                &mut Console {
                    terminal: &mut terminal(100, 30),
                    screen: &mut bare_screen(),
                },
                model,
                &mut Workers {
                    pollers,
                    updates,
                    actions: &mut actions,
                    wires: &mut wires(),
                },
                |_| {
                    thread::sleep(Duration::from_millis(20));
                    Err(PAUSED.to_string())
                },
            )
            .unwrap_err();
            if done(model) {
                return true;
            }
            if Instant::now() > end {
                return false;
            }
        }
    }

    fn reaches(model: &Model) -> Vec<(Service, shell::Reach)> {
        every_service()
            .into_iter()
            .map(|service| (service, model.reach(service)))
            .collect()
    }

    #[test]
    fn a_lab_without_a_compose_file_or_a_set_address_sends_no_request_and_shows_no_red_line() {
        let counter = Counter::start();
        let sonda = nobs_core::status::row(Service::Sonda).setting;
        let env: Vec<_> = every_setting(&counter.url)
            .into_iter()
            .filter(|(name, _)| Some(*name) != sonda)
            .collect();
        let runner = scripted(Vec::new());
        let begin = Start {
            source: Some(Arc::new(Dialing(counter.url.clone()))),
            source_service: Some(Service::Sot),
            urls: at(&counter.url, Vec::new()),
            ..start(vec![stack(&env)], false)
        };
        let (mut model, pollers, updates) = polled(begin, runner.clone(), Some(&counter.url));

        turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_millis(2500),
            |_| false,
        );
        pollers.send(Command::Quit);

        assert_eq!(counter.count(), 0);
        assert!(runner.calls().is_empty(), "{:?}", runner.calls());
        assert_eq!(model.error(), None);
        assert!(
            reaches(&model)
                .iter()
                .all(|(_, reach)| *reach == shell::Reach::NotInLab),
            "{:?}",
            reaches(&model)
        );
    }

    #[test]
    fn the_observability_workbench_of_a_lab_without_the_alerts_service_sends_no_request() {
        let counter = Counter::start();
        let alerts = nobs_core::status::row(Service::Alerts).setting.unwrap();
        let begin = Start {
            lab: Some(Located {
                dir: PathBuf::from("/ws"),
                kind: Kind::LabcliLab(None),
            }),
            nodes: None,
            topology: None,
            urls: at(&counter.url, Vec::new()),
            ..start(vec![stack(&[(alerts, counter.url.as_str())])], false)
        };
        let (mut model, pollers, updates) = polled(begin, scripted(Vec::new()), None);

        let switched = handle(&mut model, ch('3'));
        for command in &switched {
            pollers.send(command.clone());
        }
        turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_millis(2500),
            |_| false,
        );
        pollers.send(Command::Quit);

        assert_eq!(counter.count(), 0);
        assert_eq!(switched, []);
        assert_eq!(model.workbench(), Workbench::Observability);
        assert_eq!(model.reach(Service::Alerts), shell::Reach::NotInLab);
        assert_eq!(model.error(), None);
    }

    #[test]
    fn a_set_address_without_a_compose_file_is_asked_and_a_refused_one_gives_no_answer_and_the_red_line(
    ) {
        let counter = Counter::start();
        let closed = closed_url();
        let env = [
            ("PROMETHEUS_URL", counter.url.as_str()),
            ("ALERTMANAGER_URL", closed.as_str()),
        ];
        let begin = Start {
            urls: Urls {
                prometheus: counter.url.clone(),
                alertmanager: closed.clone(),
                set: vec![Service::Metrics, Service::Alerts],
                ..Urls::defaults()
            },
            ..start(vec![stack(&env)], false)
        };
        let (mut model, pollers, updates) = polled(begin, scripted(Vec::new()), None);

        let refused = turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_secs(5),
            |model| model.reach(Service::Alerts) == shell::Reach::NoAnswer,
        );
        pollers.send(Command::Quit);

        assert!(refused, "{:?}", reaches(&model));
        assert!(counter.count() > 0);
        assert!(model.error().is_some());
        assert_eq!(model.reach(Service::Logs), shell::Reach::NotInLab);
    }

    #[test]
    fn a_stack_that_names_the_service_and_is_down_sends_no_request_and_shows_no_red_line() {
        let counter = Counter::start();
        let env = every_setting(&counter.url);
        let begin = Start {
            source: Some(Arc::new(Dialing(counter.url.clone()))),
            source_service: Some(Service::Sot),
            urls: at(&counter.url, every_service()),
            ..start(vec![composed(&env)], false)
        };
        let docker = Docker::new(&compose_names(), false);
        let (mut model, pollers, updates) = polled(begin, docker, Some(&counter.url));

        turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_millis(2500),
            |_| false,
        );
        pollers.send(Command::Quit);

        assert_eq!(counter.count(), 0);
        assert_eq!(model.error(), None);
        assert!(
            reaches(&model)
                .iter()
                .all(|(_, reach)| *reach == shell::Reach::StackDown),
            "{:?}",
            reaches(&model)
        );
    }

    #[test]
    fn asking_starts_when_the_stack_comes_up_and_stops_with_its_values_and_red_line_when_it_goes_down(
    ) {
        let stub = Stub::start(&[(
            "GET",
            "/api/v2/alerts*",
            200,
            r#"[{"labels":{"alertname":"BgpSessionNotUp","device":"srl1"},"status":{"state":"active","silencedBy":[],"inhibitedBy":[]},"startsAt":"2026-11-09T13:40:00.000Z"}]"#,
        )]);
        let closed = closed_url();
        let env = [
            ("ALERTMANAGER_URL", stub.url()),
            ("PROMETHEUS_URL", closed.as_str()),
        ];
        let begin = Start {
            source: Some(Arc::new(Dialing(closed.clone()))),
            urls: Urls {
                alertmanager: stub.url().to_string(),
                prometheus: closed.clone(),
                ..Urls::defaults()
            },
            ..start(vec![composed(&env)], false)
        };
        let docker = Docker::new(&["prometheus", "alertmanager"], false);
        let (mut model, pollers, updates) = polled(begin, docker.clone(), None);
        let answered = |model: &Model| {
            model.alerts_ok()
                && model
                    .error()
                    .is_some_and(|(source, ..)| source == "metrics")
        };

        let down = turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_secs(5),
            |model| model.reach(Service::Alerts) == shell::Reach::StackDown,
        );
        let before = stub.requests().len();
        docker.up.store(true, Ordering::SeqCst);
        pollers.send(Command::Refresh);
        let up = turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_secs(5),
            answered,
        );
        let values = model.alerts().len();
        docker.up.store(false, Ordering::SeqCst);
        pollers.send(Command::Refresh);
        let down_again = turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_secs(5),
            |model| model.reach(Service::Alerts) == shell::Reach::StackDown,
        );
        let after = stub.requests().len();
        turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_millis(2500),
            |_| false,
        );
        pollers.send(Command::Quit);

        assert!(down && up && down_again, "{down} {up} {down_again}");
        assert_eq!(before, 0);
        assert_eq!(values, 1);
        assert!(model.alerts().is_empty());
        assert!(!model.alerts_ok());
        assert_eq!(model.error(), None);
        assert_eq!(stub.requests().len(), after);
    }

    #[test]
    fn nothing_is_asked_until_the_first_docker_answer_about_the_stack() {
        let counter = Counter::start();
        let env = every_setting(&counter.url);
        let begin = Start {
            urls: at(&counter.url, every_service()),
            ..start(vec![composed(&env)], false)
        };
        let docker = Arc::new(Docker {
            sleep: Duration::from_secs(3),
            ..Arc::into_inner(Docker::new(&compose_names(), true)).unwrap()
        });
        let (mut model, pollers, updates) = polled(begin, docker, Some(&counter.url));

        turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_millis(1500),
            |_| false,
        );
        pollers.send(Command::Quit);

        assert_eq!(counter.count(), 0);
        assert_eq!(model.error(), None);
        assert!(
            reaches(&model)
                .iter()
                .all(|(_, reach)| *reach == shell::Reach::Unknown),
            "{:?}",
            reaches(&model)
        );
    }

    #[test]
    fn the_health_of_a_running_infrahub_prefect_and_sonda_server_shows() {
        let stub = Stub::start(&[("GET", "/health", 200, ""), ("GET", "/api/health", 200, "")]);
        let env = [
            ("INFRAHUB_ADDRESS", stub.url()),
            ("PREFECT_API_URL", stub.url()),
            ("SONDA_SERVER_URL", stub.url()),
        ];
        let names = ["infrahub-server", "prefect-server", "sonda-server"];
        let begin = start(vec![composed(&env)], false);
        let (mut model, pollers, updates) = polled(begin, Docker::new(&names, true), None);
        let healths = |model: &Model| -> Vec<Option<String>> {
            names
                .iter()
                .map(|name| {
                    model
                        .services()
                        .iter()
                        .find(|service| service.name == *name)
                        .and_then(|service| service.health.clone())
                })
                .collect()
        };

        turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_secs(5),
            |model| healths(model).iter().all(Option::is_some),
        );
        pollers.send(Command::Quit);

        assert_eq!(healths(&model), vec![Some("ok".to_string()); 3]);
    }

    fn counted_by_the_pollers_only(begin: Start, names: &[&'static str]) -> (Model, bool) {
        let counted = Counter::start();
        let (mut model, pollers, updates) =
            polled(begin, Docker::new(names, true), Some(&counted.url));
        let asked = turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_millis(2500),
            |_| counted.count() > 0,
        );
        pollers.send(Command::Quit);
        (model, asked)
    }

    #[test]
    fn outside_a_lab_a_stack_without_sonda_and_nothing_set_does_not_ask_sonda() {
        let begin = start(vec![composed(&[])], true);

        let (model, asked) = counted_by_the_pollers_only(begin, &["prometheus"]);

        assert_eq!(model.reach(Service::Sonda), shell::Reach::NotInLab);
        assert!(!asked);
    }

    #[test]
    fn outside_a_lab_a_stack_whose_env_sets_the_sonda_address_is_asked_there() {
        let counter = Counter::start();
        let sonda = nobs_core::status::row(Service::Sonda).setting.unwrap();
        let begin = start(vec![composed(&[(sonda, counter.url.as_str())])], true);
        let (mut model, pollers, updates) = polled(begin, Docker::new(&["prometheus"], true), None);

        let asked = turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_millis(2500),
            |_| counter.count() > 0,
        );
        pollers.send(Command::Quit);

        assert_eq!(model.reach(Service::Sonda), shell::Reach::Answers);
        assert!(asked);
    }

    #[test]
    fn in_a_lab_the_sonda_address_counts_as_set_only_from_the_env_of_the_selected_stack() {
        let counter = Counter::start();
        let sonda = nobs_core::status::row(Service::Sonda).setting.unwrap();
        let other = Stack {
            name: "other".to_string(),
            path: PathBuf::from("/other"),
            ..composed(&[(sonda, counter.url.as_str())])
        };
        let begin = start(vec![composed(&[]), other], false);
        let (mut model, pollers, updates) = polled(begin, Docker::new(&["prometheus"], true), None);

        turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_millis(1500),
            |_| false,
        );
        let at_the_first = (model.reach(Service::Sonda), counter.count());
        let moved = handle(&mut model, key(KeyCode::Char('j')));
        for command in &moved {
            pollers.send(command.clone());
        }
        let asked = turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_millis(2500),
            |_| counter.count() > 0,
        );
        pollers.send(Command::Quit);

        assert_eq!(at_the_first, (shell::Reach::NotInLab, 0));
        assert_eq!(moved, [Command::SelectStack(1)]);
        assert!(asked);
        assert_eq!(model.sonda().server(), counter.url);
        assert!(
            matches!(
                model.reach(Service::Sonda),
                shell::Reach::Answers | shell::Reach::NoAnswer
            ),
            "{:?}",
            model.reach(Service::Sonda)
        );
    }

    #[test]
    fn the_sonda_only_stack_is_asked_with_nothing_set() {
        let sonda_only = Stack {
            kind: StackKind::Sonda,
            shows_sonda: true,
            ..stack(&[])
        };
        let begin = start(vec![sonda_only], true);

        let (model, asked) = counted_by_the_pollers_only(begin, &[]);

        assert_eq!(model.reach(Service::Sonda), shell::Reach::Answers);
        assert!(asked);
    }

    #[test]
    fn the_sonda_health_check_follows_the_server_flag() {
        let stub = Stub::start(&[("GET", "/health", 200, "")]);
        let begin = Start {
            sonda: SondaTarget {
                server: Some(stub.url().to_string()),
                api_key: None,
            },
            ..start(vec![composed(&[])], false)
        };
        let (mut model, pollers, updates) = polled(
            begin,
            Docker::new(&["sonda-server"], true),
            Some(stub.url()),
        );
        let health = |model: &Model| {
            model
                .services()
                .iter()
                .find(|service| service.name == "sonda-server")
                .and_then(|service| service.health.clone())
        };

        turn_until(
            &mut model,
            &pollers,
            &updates,
            Duration::from_secs(5),
            |model| health(model).is_some(),
        );
        pollers.send(Command::Quit);

        assert_eq!(health(&model), Some("ok".to_string()));
    }

    #[test]
    fn the_loop_measure_gives_the_page_of_the_drawn_box() {
        let mut model = devices_model();
        let names = (1..=40).map(|index| device(&format!("d{index:02}")));
        model.apply(Update::Devices(Ok(names.collect())));
        assert_eq!(model.focus(), Focus::List);

        measure(&mut model, Rect::new(0, 0, 60, 15));
        assert_eq!(model.page(), 4);
        handle(&mut model, key(KeyCode::PageDown));
        assert_eq!(model.cursor(), shell::Cursor::Device(4));

        measure(&mut model, Rect::new(0, 0, 100, 30));
        assert_eq!(model.page(), 15);
        handle(&mut model, key(KeyCode::PageDown));
        assert_eq!(model.cursor(), shell::Cursor::Device(19));
    }

    #[test]
    fn a_turn_measures_the_drawn_box_before_the_next_key() {
        let mut model = devices_model();
        let names = (1..=40).map(|index| device(&format!("d{index:02}")));
        model.apply(Update::Devices(Ok(names.collect())));
        let (pollers, updates) = launch(&scripted(Vec::new()), &model);
        let mut terminal = terminal(100, 30);
        let mut pressed = [key(KeyCode::PageDown), q()].into_iter();

        turns(
            &mut Console {
                terminal: &mut terminal,
                screen: &mut bare_screen(),
            },
            &mut model,
            &mut Workers {
                pollers: &pollers,
                updates: &updates,
                actions: &mut actions(),
                wires: &mut wires(),
            },
            |_| Ok(pressed.next()),
        )
        .unwrap();
        let drawn = text(&terminal);
        let shown = (1..=40)
            .filter(|index| drawn.contains(&format!("d{index:02} ")))
            .count();

        assert_eq!(model.page(), shown - 1);
        assert_eq!(model.cursor(), shell::Cursor::Device(model.page()));
    }
}
