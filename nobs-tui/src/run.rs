use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nobs_core::runner::exit_code;

use crate::shell::Update;

const TICK: Duration = Duration::from_millis(50);
const PIPES_GRACE: Duration = Duration::from_secs(1);
const KILL_AFTER: Duration = Duration::from_secs(5);
const KILL_GRACE: Duration = Duration::from_secs(2);

pub trait Launcher: Send + Sync {
    fn start(
        &self,
        program: &Path,
        args: &[OsString],
        out: Sender<Update>,
    ) -> io::Result<Box<dyn Running>>;
}

pub trait Running: Send {
    fn stop(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Exited(i32),
    Stopped,
    NotStopped { group: u32 },
}

pub struct Processes;

#[derive(Default)]
struct State {
    stopped: Option<Instant>,
    ended: bool,
}

struct Process {
    group: u32,
    state: Arc<Mutex<State>>,
}

impl Launcher for Processes {
    fn start(
        &self,
        program: &Path,
        args: &[OsString],
        out: Sender<Update>,
    ) -> io::Result<Box<dyn Running>> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let readers = [
            read(child.stdout.take().expect("stdout is piped"), out.clone()),
            read(child.stderr.take().expect("stderr is piped"), out.clone()),
        ];
        let group = child.id();
        let state = Arc::new(Mutex::new(State::default()));
        let watched = Arc::clone(&state);
        thread::spawn(move || watch(child, readers, &watched, &out));
        Ok(Box::new(Process { group, state }))
    }
}

impl Running for Process {
    fn stop(&mut self) {
        let mut state = self.state.lock().unwrap();
        if state.ended || state.stopped.is_some() {
            return;
        }
        let _ = signal(self.group, libc::SIGINT);
        state.stopped = Some(Instant::now());
    }
}

fn read(source: impl Read + Send + 'static, out: Sender<Update>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(source);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            if !matches!(reader.read_until(b'\n', &mut bytes), Ok(read) if read > 0) {
                return;
            }
            let line = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let line = String::from_utf8_lossy(line).into_owned();
            if out.send(Update::ActionLine(line)).is_err() {
                return;
            }
        }
    })
}

fn watch(
    mut child: Child,
    readers: [JoinHandle<()>; 2],
    state: &Mutex<State>,
    out: &Sender<Update>,
) {
    let group = child.id();
    let mut exited: Option<(i32, Instant)> = None;
    let mut killed: Option<Instant> = None;
    let end = loop {
        thread::sleep(TICK);
        if exited.is_none() {
            exited = child
                .try_wait()
                .ok()
                .flatten()
                .map(|status| (exit_code(status), Instant::now()));
        }
        let mut held = state.lock().unwrap();
        let end = match held.stopped {
            None => exited
                .filter(|(_, at)| {
                    readers.iter().all(JoinHandle::is_finished) || at.elapsed() >= PIPES_GRACE
                })
                .map(|(code, _)| End::Exited(code)),
            Some(_) if exited.is_some() && group_gone(signal(group, 0)) => Some(End::Stopped),
            Some(_) if killed.is_some_and(|at| at.elapsed() >= KILL_GRACE) => {
                Some(End::NotStopped { group })
            }
            Some(stopped) => {
                if killed.is_none() && stopped.elapsed() >= KILL_AFTER {
                    let _ = signal(group, libc::SIGKILL);
                    killed = Some(Instant::now());
                }
                None
            }
        };
        if let Some(end) = end {
            held.ended = true;
            break end;
        }
    };
    let _ = out.send(Update::ActionEnded(end));
    if exited.is_none() {
        let _ = child.wait();
    }
}

fn signal(group: u32, signal: libc::c_int) -> io::Result<()> {
    if unsafe { libc::killpg(group as libc::pid_t, signal) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn group_gone(result: io::Result<()>) -> bool {
    result.is_err_and(|error| error.raw_os_error() == Some(libc::ESRCH))
}

#[derive(Default)]
pub struct FakeLauncher {
    failing: bool,
    calls: Mutex<Vec<(PathBuf, Vec<OsString>)>>,
    stops: Arc<AtomicUsize>,
    sender: Mutex<Option<Sender<Update>>>,
}

struct FakeRunning {
    stops: Arc<AtomicUsize>,
    out: Sender<Update>,
}

impl FakeLauncher {
    pub fn new() -> FakeLauncher {
        FakeLauncher::default()
    }

    pub fn failing() -> FakeLauncher {
        FakeLauncher {
            failing: true,
            ..FakeLauncher::default()
        }
    }

    pub fn calls(&self) -> Vec<(PathBuf, Vec<OsString>)> {
        self.calls.lock().unwrap().clone()
    }

    pub fn stops(&self) -> usize {
        self.stops.load(Ordering::SeqCst)
    }

    pub fn sender(&self) -> Option<Sender<Update>> {
        self.sender.lock().unwrap().clone()
    }
}

impl Launcher for FakeLauncher {
    fn start(
        &self,
        program: &Path,
        args: &[OsString],
        out: Sender<Update>,
    ) -> io::Result<Box<dyn Running>> {
        self.calls
            .lock()
            .unwrap()
            .push((program.to_path_buf(), args.to_vec()));
        if self.failing {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "fake launcher failure",
            ));
        }
        *self.sender.lock().unwrap() = Some(out.clone());
        Ok(Box::new(FakeRunning {
            stops: Arc::clone(&self.stops),
            out,
        }))
    }
}

impl Running for FakeRunning {
    fn stop(&mut self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
        let _ = self.out.send(Update::ActionEnded(End::Stopped));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn shell(script: &str) -> (Box<dyn Running>, mpsc::Receiver<Update>) {
        let (out, updates) = mpsc::channel();
        let args = ["-c", script].map(OsString::from);
        let running = Processes.start(Path::new("sh"), &args, out).unwrap();
        (running, updates)
    }

    #[test]
    fn a_line_arrives_while_the_child_still_runs() {
        let (_running, updates) = shell("echo one; sleep 5; echo two");

        let first = updates.recv_timeout(Duration::from_secs(3)).unwrap();

        assert!(
            matches!(&first, Update::ActionLine(line) if line == "one"),
            "{first:?}"
        );
    }

    #[test]
    fn the_end_carries_the_exit_code_of_the_child() {
        let (_running, updates) = shell("exit 3");

        let end = updates.recv_timeout(Duration::from_secs(5)).unwrap();

        assert!(
            matches!(end, Update::ActionEnded(End::Exited(3))),
            "{end:?}"
        );
    }

    #[test]
    fn stopping_ends_the_whole_group_with_its_background_jobs() {
        let (mut running, updates) = shell("sleep 300 & echo $!; sleep 300");
        let printed = updates.recv_timeout(Duration::from_secs(5)).unwrap();
        let Update::ActionLine(pid) = &printed else {
            panic!("{printed:?}");
        };
        let pid: libc::pid_t = pid.parse().unwrap();

        running.stop();
        let end = updates.recv_timeout(Duration::from_secs(15)).unwrap();

        assert!(matches!(end, Update::ActionEnded(End::Stopped)), "{end:?}");
        let gone = || {
            (unsafe { libc::kill(pid, 0) }) != 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gone() && Instant::now() < deadline {
            thread::sleep(TICK);
        }
        assert!(gone(), "process {pid} is still there");
    }

    #[test]
    fn only_esrch_means_the_group_is_gone() {
        let error = |code| Err(io::Error::from_raw_os_error(code));

        assert!(group_gone(error(libc::ESRCH)));
        assert!(!group_gone(error(libc::EPERM)));
        assert!(!group_gone(Ok(())));
    }
}
