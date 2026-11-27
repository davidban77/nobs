use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::run::{login_vars, Invocation};
use crate::shell::Update;

const REAP_PAUSE: Duration = Duration::from_millis(5);
const HANGUP_GRACE: Duration = Duration::from_millis(200);

pub trait Terminals: Send + Sync {
    fn open(
        &self,
        child: Invocation<'_>,
        dir: &Path,
        size: (u16, u16),
        id: u64,
        out: Sender<Update>,
    ) -> io::Result<Box<dyn Wire>>;
}

pub trait Wire: Send {
    fn write(&mut self, bytes: &[u8]);
    fn resize(&mut self, size: (u16, u16));
    fn close(&mut self);
}

pub struct Ptys;

struct Pty {
    master: Box<dyn MasterPty + Send>,
    writes: Option<Sender<Vec<u8>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    reaped: Arc<Mutex<bool>>,
    wake: Option<UnixStream>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}

impl Terminals for Ptys {
    fn open(
        &self,
        child: Invocation<'_>,
        dir: &Path,
        size: (u16, u16),
        id: u64,
        out: Sender<Update>,
    ) -> io::Result<Box<dyn Wire>> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(io::Error::other)?;
        let mut output = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let input = pair.master.take_writer().map_err(io::Error::other)?;
        let fd = pair
            .master
            .as_raw_fd()
            .ok_or_else(|| io::Error::other("the terminal has no file"))?;
        set_nonblocking(fd)?;
        let (wake, woken) = UnixStream::pair()?;
        woken.set_nonblocking(true)?;
        let writer_woken = woken.try_clone()?;
        let mut command = CommandBuilder::new(child.program);
        command.args(child.args);
        command.cwd(dir);
        command.env("TERM", "xterm");
        for (name, value) in login_vars(child.login) {
            command.env(name, value);
        }
        let mut process = pair
            .slave
            .spawn_command(command)
            .map_err(io::Error::other)?;
        drop(pair.slave);
        let killer = process.clone_killer();
        let reaped = Arc::new(Mutex::new(false));
        let reaping = Arc::clone(&reaped);
        let reader = thread::spawn(move || {
            let mut buffer = [0; 4096];
            let mut open = true;
            let mut closed = None;
            let code = loop {
                if closed.is_none() && is_woken(&woken) {
                    closed = Some(Instant::now());
                }
                let got = if open {
                    read_once(&mut *output, &mut buffer)
                } else {
                    Got::End
                };
                match got {
                    Got::Bytes(read) => {
                        let _ = out.send(Update::TermOutput(id, buffer[..read].to_vec()));
                    }
                    Got::Nothing => {}
                    Got::End => open = false,
                }
                if open && closed.is_none() {
                    if got == Got::Nothing {
                        wait_for_data(fd, &woken);
                    }
                    continue;
                }
                if let Some(code) = reap(&mut *process, &reaping, closed) {
                    break code;
                }
                if !matches!(got, Got::Bytes(_)) {
                    thread::sleep(REAP_PAUSE);
                }
            };
            let _ = out.send(Update::TermEnded(id, code));
        });
        let (writes, writer) = spawn_writer(input, fd, writer_woken);
        Ok(Box::new(Pty {
            master: pair.master,
            writes: Some(writes),
            killer,
            reaped,
            wake: Some(wake),
            reader: Some(reader),
            writer: Some(writer),
        }))
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Got {
    Bytes(usize),
    Nothing,
    End,
}

fn read_once(output: &mut dyn Read, buffer: &mut [u8]) -> Got {
    match output.read(buffer) {
        Ok(read @ 1..) => Got::Bytes(read),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Got::Nothing,
        _ => Got::End,
    }
}

fn is_woken(mut woken: &UnixStream) -> bool {
    matches!(woken.read(&mut [0]), Ok(0))
}

fn reap(
    process: &mut (dyn Child + Send + Sync),
    reaped: &Mutex<bool>,
    closed: Option<Instant>,
) -> Option<i32> {
    let mut reaped = reaped.lock().unwrap_or_else(PoisonError::into_inner);
    match process.try_wait() {
        Ok(None) => {
            if let (Some(at), Some(pid)) = (closed, process.process_id()) {
                if at.elapsed() >= HANGUP_GRACE {
                    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
                }
            }
            None
        }
        waited => {
            *reaped = true;
            Some(
                waited
                    .ok()
                    .flatten()
                    .map_or(1, |status| status.exit_code() as i32),
            )
        }
    }
}

fn wait_for_data(fd: RawFd, woken: &UnixStream) {
    poll_all(&mut [
        watch(fd, libc::POLLIN),
        watch(woken.as_raw_fd(), libc::POLLIN),
    ]);
}

fn wait_for_room(fd: RawFd, woken: &UnixStream) -> bool {
    let mut fds = [
        watch(fd, libc::POLLOUT),
        watch(woken.as_raw_fd(), libc::POLLIN),
    ];
    poll_all(&mut fds) && fds[1].revents == 0 && fds[0].revents & libc::POLLOUT != 0
}

fn watch(fd: RawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd {
        fd,
        events,
        revents: 0,
    }
}

fn poll_all(fds: &mut [libc::pollfd]) -> bool {
    while unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) } == -1 {
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
    true
}

fn write_whole(input: &mut dyn Write, mut bytes: &[u8], fd: RawFd, woken: &UnixStream) -> bool {
    while !bytes.is_empty() {
        match input.write(bytes) {
            Ok(0) => return false,
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && wait_for_room(fd, woken) => {
            }
            Err(_) => return false,
        }
    }
    input.flush().is_ok()
}

fn spawn_writer(
    mut input: Box<dyn Write + Send>,
    fd: RawFd,
    woken: UnixStream,
) -> (Sender<Vec<u8>>, JoinHandle<()>) {
    let (writes, queue) = mpsc::channel::<Vec<u8>>();
    let writer = thread::spawn(move || {
        for bytes in queue {
            if !write_whole(&mut *input, &bytes, fd, &woken) {
                return;
            }
        }
    });
    (writes, writer)
}

fn pty_size((rows, cols): (u16, u16)) -> PtySize {
    PtySize {
        rows,
        cols,
        ..PtySize::default()
    }
}

impl Wire for Pty {
    fn write(&mut self, bytes: &[u8]) {
        if let Some(writes) = &self.writes {
            let _ = writes.send(bytes.to_vec());
        }
    }

    fn resize(&mut self, size: (u16, u16)) {
        let _ = self.master.resize(pty_size(size));
    }

    fn close(&mut self) {
        let reaped = self.reaped.lock().unwrap_or_else(PoisonError::into_inner);
        if !*reaped {
            let _ = self.killer.kill();
        }
        drop(reaped);
        self.writes = None;
        self.wake = None;
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.close();
        for thread in [self.reader.take(), self.writer.take()]
            .into_iter()
            .flatten()
        {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use nobs_core::device;

    use super::*;
    use crate::shell::Login;

    const WAIT: Duration = Duration::from_secs(5);

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Call {
        Open {
            id: u64,
            program: PathBuf,
            args: Vec<OsString>,
            dir: PathBuf,
            size: (u16, u16),
            login: Login,
        },
        Write(u64, Vec<u8>),
        Resize(u64, (u16, u16)),
        Close(u64),
    }

    #[derive(Default)]
    pub(crate) struct FakeTerminals {
        calls: Arc<Mutex<Vec<Call>>>,
        sender: Mutex<Option<Sender<Update>>>,
    }

    struct FakeWire {
        id: u64,
        calls: Arc<Mutex<Vec<Call>>>,
    }

    impl FakeTerminals {
        pub(crate) fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        pub(crate) fn sender(&self) -> Option<Sender<Update>> {
            self.sender.lock().unwrap().clone()
        }
    }

    impl Terminals for FakeTerminals {
        fn open(
            &self,
            child: Invocation<'_>,
            dir: &Path,
            size: (u16, u16),
            id: u64,
            out: Sender<Update>,
        ) -> io::Result<Box<dyn Wire>> {
            self.calls.lock().unwrap().push(Call::Open {
                id,
                program: child.program.to_path_buf(),
                args: child.args.to_vec(),
                dir: dir.to_path_buf(),
                size,
                login: child.login.clone(),
            });
            *self.sender.lock().unwrap() = Some(out);
            Ok(Box::new(FakeWire {
                id,
                calls: Arc::clone(&self.calls),
            }))
        }
    }

    impl Wire for FakeWire {
        fn write(&mut self, bytes: &[u8]) {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Write(self.id, bytes.to_vec()));
        }

        fn resize(&mut self, size: (u16, u16)) {
            self.calls.lock().unwrap().push(Call::Resize(self.id, size));
        }

        fn close(&mut self) {
            self.calls.lock().unwrap().push(Call::Close(self.id));
        }
    }

    fn open(args: &[&str], size: (u16, u16), login: &Login) -> (Box<dyn Wire>, Receiver<Update>) {
        let (out, updates) = mpsc::channel();
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let wire = Ptys
            .open(
                Invocation {
                    program: Path::new("/bin/sh"),
                    args: &args,
                    login,
                },
                Path::new(env!("CARGO_MANIFEST_DIR")),
                size,
                7,
                out,
            )
            .unwrap();
        (wire, updates)
    }

    fn output_until(updates: &Receiver<Update>, text: &str) -> String {
        let mut seen = String::new();
        while !seen.contains(text) {
            match updates.recv_timeout(WAIT) {
                Ok(Update::TermOutput(7, bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                other => panic!("{other:?} after {seen:?}"),
            }
        }
        seen
    }

    fn end(updates: &Receiver<Update>) -> (String, i32) {
        let mut seen = String::new();
        loop {
            match updates.recv_timeout(WAIT) {
                Ok(Update::TermOutput(7, bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                Ok(Update::TermEnded(7, code)) => return (seen, code),
                other => panic!("{other:?} after {seen:?}"),
            }
        }
    }

    #[test]
    fn bytes_written_reach_the_shell_and_its_echo_comes_back() {
        let (mut wire, updates) = open(
            &["-c", "read line; echo \"got $line\""],
            (24, 80),
            &Login::default(),
        );

        wire.write(b"nobs-t-hello\r");
        let (seen, code) = end(&updates);

        assert_eq!(seen.matches("nobs-t-hello").count(), 2, "{seen:?}");
        assert!(seen.contains("got nobs-t-hello"), "{seen:?}");
        assert_eq!(code, 0);
    }

    #[test]
    fn the_child_starts_at_the_given_size_and_follows_a_resize() {
        let (mut wire, updates) = open(
            &["-c", "stty size; read line; stty size"],
            (24, 80),
            &Login::default(),
        );
        let first = output_until(&updates, "24 80");

        wire.resize((13, 78));
        wire.write(b"\r");
        let (seen, _) = end(&updates);

        assert!(first.contains("24 80"), "{first:?}");
        assert!(seen.contains("13 78"), "{seen:?}");
    }

    #[test]
    fn the_end_of_the_child_carries_its_status() {
        let (_wire, updates) = open(&["-c", "exit 3"], (24, 80), &Login::default());

        let (_, code) = end(&updates);

        assert_eq!(code, 3);
    }

    #[test]
    fn after_close_the_child_is_gone() {
        let (mut wire, updates) = open(
            &["-c", "echo pid=$$; exec sleep 10"],
            (24, 80),
            &Login::default(),
        );
        let seen = output_until(&updates, "\n");
        let pid: libc::pid_t = seen
            .trim()
            .strip_prefix("pid=")
            .and_then(|pid| pid.parse().ok())
            .unwrap_or_else(|| panic!("{seen:?}"));

        wire.close();
        end(&updates);
        drop(wire);

        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn a_large_write_to_a_child_that_never_reads_returns_at_once_and_close_leaves_nothing() {
        let (mut wire, updates) = open(
            &["-c", "stty raw -echo; echo pid=$$; exec sleep 10"],
            (24, 80),
            &Login::default(),
        );
        let seen = output_until(&updates, "\n");
        let pid: libc::pid_t = seen
            .trim()
            .strip_prefix("pid=")
            .and_then(|pid| pid.parse().ok())
            .unwrap_or_else(|| panic!("{seen:?}"));

        let started = Instant::now();
        wire.write(&[b'x'; 64 * 1024]);
        let took = started.elapsed();
        wire.close();
        drop(wire);

        assert!(took < Duration::from_millis(100), "{took:?}");
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        let left: Vec<Update> = updates.try_iter().collect();
        assert!(
            matches!(left.last(), Some(Update::TermEnded(7, _))),
            "{left:?}"
        );
        assert!(matches!(
            updates.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    fn pid_in(seen: &str) -> libc::pid_t {
        seen.trim()
            .strip_prefix("pid=")
            .and_then(|pid| pid.parse().ok())
            .unwrap_or_else(|| panic!("{seen:?}"))
    }

    fn gone_within(pid: libc::pid_t, bound: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < bound {
            if unsafe { libc::kill(pid, 0) } == -1
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return true;
            }
            thread::sleep(REAP_PAUSE);
        }
        false
    }

    #[test]
    fn close_ends_in_time_while_a_grandchild_that_ignores_the_hangup_holds_the_terminal() {
        let (mut wire, updates) = open(
            &[
                "-c",
                "(trap '' HUP; exec sleep 10) & echo pid=$!; exec sleep 10",
            ],
            (24, 80),
            &Login::default(),
        );
        let grandchild = pid_in(&output_until(&updates, "\n"));

        let started = Instant::now();
        wire.close();
        drop(wire);
        let took = started.elapsed();
        unsafe { libc::kill(grandchild, libc::SIGKILL) };

        assert!(took < Duration::from_secs(2), "{took:?}");
        let left: Vec<Update> = updates.try_iter().collect();
        assert!(
            matches!(left.last(), Some(Update::TermEnded(7, _))),
            "{left:?}"
        );
        assert!(gone_within(grandchild, WAIT));
    }

    #[test]
    fn close_ends_in_time_a_child_that_ignores_the_hangup() {
        let (mut wire, updates) = open(
            &["-c", "trap '' HUP; echo pid=$$; exec sleep 10"],
            (24, 80),
            &Login::default(),
        );
        let pid = pid_in(&output_until(&updates, "\n"));

        let started = Instant::now();
        wire.close();
        drop(wire);
        let took = started.elapsed();

        assert!(took < Duration::from_secs(2), "{took:?}");
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn a_child_that_ends_on_the_hangup_is_never_killed() {
        let (mut wire, updates) = open(
            &[
                "-c",
                "trap 'sleep 0.05; exit 5' HUP; echo ready; while :; do :; done",
            ],
            (24, 80),
            &Login::default(),
        );
        output_until(&updates, "ready");

        wire.close();
        drop(wire);

        let left: Vec<Update> = updates.try_iter().collect();
        assert!(
            matches!(left.last(), Some(Update::TermEnded(7, 5))),
            "{left:?}"
        );
    }

    #[test]
    fn a_write_larger_than_the_terminal_holds_reaches_a_reading_child_whole() {
        let (mut wire, updates) = open(
            &["-c", "stty raw -echo; echo ready; head -c 262144 | wc -c"],
            (24, 80),
            &Login::default(),
        );
        output_until(&updates, "ready");

        wire.write(&[b'x'; 262_144]);
        let (seen, code) = end(&updates);

        assert!(seen.contains("262144"), "{seen:?}");
        assert_eq!(code, 0);
    }

    #[derive(Debug, Clone, Default)]
    struct Killer(Arc<AtomicUsize>);

    impl ChildKiller for Killer {
        fn kill(&mut self) -> io::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(self.clone())
        }
    }

    struct Slow(Arc<AtomicBool>);

    impl Write for Slow {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            thread::sleep(Duration::from_millis(200));
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Slow {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn bare(killer: Killer, reaped: bool, input: Box<dyn Write + Send>) -> Pty {
        let pair = native_pty_system().openpty(pty_size((24, 80))).unwrap();
        let (wake, woken) = UnixStream::pair().unwrap();
        let (writes, writer) = spawn_writer(input, pair.master.as_raw_fd().unwrap(), woken);
        Pty {
            master: pair.master,
            writes: Some(writes),
            killer: Box::new(killer),
            reaped: Arc::new(Mutex::new(reaped)),
            wake: Some(wake),
            reader: None,
            writer: Some(writer),
        }
    }

    #[test]
    fn close_signals_a_child_not_yet_reaped_and_never_one_already_reaped() {
        let mut kills = Vec::new();
        for reaped in [false, true] {
            let killer = Killer::default();
            let mut pty = bare(killer.clone(), reaped, Box::new(io::sink()));

            pty.close();

            kills.push(killer.0.load(Ordering::SeqCst));
        }

        assert_eq!(kills, [1, 0]);
    }

    #[test]
    fn dropping_the_wire_waits_for_its_writer_to_end() {
        let ended = Arc::new(AtomicBool::new(false));
        let mut pty = bare(Killer::default(), true, Box::new(Slow(Arc::clone(&ended))));

        pty.write(b"nobs-t-x");
        drop(pty);

        assert!(ended.load(Ordering::SeqCst));
    }

    #[test]
    fn the_login_reaches_the_environment_of_the_child_and_no_argument() {
        let login = Login {
            user: Some("nobs-t-x".to_string()),
            password: Some(device::Password::new("nobs-t-not-a-secret".to_string())),
        };
        let script = "printf 'args=%s|\\n' \"$*\"; pwd; echo \"TERM=$TERM\"; \
            [ \"$NETWORK_AGENT_USER\" = nobs-t-x ] && echo user; \
            [ \"$NETWORK_AGENT_PASSWORD\" = nobs-t-not-a-secret ] && echo password; true";

        let (_wire, updates) = open(&["-c", script, "nobs-t-zero"], (24, 80), &login);
        let (seen, code) = end(&updates);
        let lines: Vec<&str> = seen
            .lines()
            .map(|line| line.trim_end_matches('\r'))
            .collect();

        assert_eq!(code, 0);
        assert!(lines.contains(&"args=|"), "{lines:?}");
        assert!(lines.contains(&env!("CARGO_MANIFEST_DIR")), "{lines:?}");
        assert!(lines.contains(&"TERM=xterm"), "{lines:?}");
        assert!(lines.contains(&"user"), "{lines:?}");
        assert!(lines.contains(&"password"), "{lines:?}");
    }
}
