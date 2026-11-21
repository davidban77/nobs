use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::run::login_vars;
use crate::shell::{Login, Update};

pub trait Terminals: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn open(
        &self,
        program: &Path,
        args: &[OsString],
        dir: &Path,
        login: &Login,
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
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    reader: Option<JoinHandle<()>>,
}

impl Terminals for Ptys {
    fn open(
        &self,
        program: &Path,
        args: &[OsString],
        dir: &Path,
        login: &Login,
        size: (u16, u16),
        id: u64,
        out: Sender<Update>,
    ) -> io::Result<Box<dyn Wire>> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(io::Error::other)?;
        let mut output = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let writer = pair.master.take_writer().map_err(io::Error::other)?;
        let mut command = CommandBuilder::new(program);
        command.args(args);
        command.cwd(dir);
        command.env("TERM", "xterm");
        for (name, value) in login_vars(login) {
            command.env(name, value);
        }
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(io::Error::other)?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let reader = thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(read @ 1..) = output.read(&mut buffer) {
                let _ = out.send(Update::TermOutput(id, buffer[..read].to_vec()));
            }
            let code = child.wait().map_or(1, |status| status.exit_code() as i32);
            let _ = out.send(Update::TermEnded(id, code));
        });
        Ok(Box::new(Pty {
            master: pair.master,
            writer,
            killer,
            reader: Some(reader),
        }))
    }
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
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    fn resize(&mut self, size: (u16, u16)) {
        let _ = self.master.resize(pty_size(size));
    }

    fn close(&mut self) {
        if self
            .reader
            .as_ref()
            .is_some_and(|reader| !reader.is_finished())
        {
            let _ = self.killer.kill();
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.close();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use nobs_core::device;

    use super::*;

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
            program: &Path,
            args: &[OsString],
            dir: &Path,
            login: &Login,
            size: (u16, u16),
            id: u64,
            out: Sender<Update>,
        ) -> io::Result<Box<dyn Wire>> {
            self.calls.lock().unwrap().push(Call::Open {
                id,
                program: program.to_path_buf(),
                args: args.to_vec(),
                dir: dir.to_path_buf(),
                size,
                login: login.clone(),
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
                Path::new("/bin/sh"),
                &args,
                Path::new(env!("CARGO_MANIFEST_DIR")),
                login,
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
