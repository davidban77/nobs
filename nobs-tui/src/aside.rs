use std::io::{self, Write};
use std::{mem, ptr};

use crossterm::execute;
use crossterm::terminal::EnterAlternateScreen;

use crate::leave;

pub trait Asker: Send + Sync {
    fn ask(&self) -> io::Result<i32>;
}

pub(crate) fn ask_aside<W: Write, R: FnMut(bool) -> io::Result<()>>(
    out: &mut W,
    raw: &mut R,
    lines: &[String],
    asker: &dyn Asker,
) -> io::Result<io::Result<i32>> {
    let interrupts = Interrupts::hold()?;
    leave(out, raw)?;
    for line in lines {
        writeln!(out, "{line}")?;
    }
    out.flush()?;
    let asked = asker.ask();
    raw(true)?;
    discard_typed()?;
    execute!(out, EnterAlternateScreen)?;
    drop(interrupts);
    Ok(asked)
}

fn discard_typed() -> io::Result<()> {
    if unsafe { libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

struct Interrupts(Vec<(libc::c_int, libc::sigaction)>);

extern "C" fn nothing(_: libc::c_int) {}

impl Interrupts {
    fn hold() -> io::Result<Interrupts> {
        let mut action: libc::sigaction = unsafe { mem::zeroed() };
        // A handler, not ignore: exec resets a handler, so the asking child still ends at Ctrl-C and Ctrl-\.
        action.sa_sigaction = nothing as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        let mut held = Interrupts(Vec::new());
        for signal in [libc::SIGINT, libc::SIGQUIT] {
            let mut previous: libc::sigaction = unsafe { mem::zeroed() };
            if unsafe { libc::sigaction(signal, &action, &mut previous) } != 0 {
                return Err(io::Error::last_os_error());
            }
            held.0.push((signal, previous));
        }
        Ok(held)
    }
}

impl Drop for Interrupts {
    fn drop(&mut self) {
        for (signal, previous) in &self.0 {
            unsafe { libc::sigaction(*signal, previous, ptr::null_mut()) };
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::mpsc::{self, Receiver};
    use std::sync::Mutex;
    use std::time::Duration;

    use crossterm::event;
    use nobs_core::runner::{RealRunner, Runner};

    use super::*;
    use crate::pty::{Ptys, Terminals, Wire};
    use crate::shell::{Login, Update};
    use crate::{raw_mode, Screen};

    const CHILD: &str = "nobs-t-child";
    const WAIT: Duration = Duration::from_secs(10);
    const LINES: [&str; 2] = ["nobs-t-first", "nobs-t-second"];
    const CHECK: &str = "printf 'Password: '; read word; [ \"$word\" = nobs-t-not-a-secret ]";

    pub(crate) fn is_child() -> bool {
        std::env::args().any(|arg| arg == CHILD)
    }

    pub(crate) struct Child {
        wire: Box<dyn Wire>,
        updates: Receiver<Update>,
        seen: String,
    }

    impl Child {
        pub(crate) fn start(test: &str) -> Child {
            let (out, updates) = mpsc::channel();
            let args: Vec<OsString> = [test, "--exact", "--nocapture", "--test-threads=1", CHILD]
                .map(OsString::from)
                .to_vec();
            let wire = Ptys
                .open(
                    &std::env::current_exe().unwrap(),
                    &args,
                    Path::new(env!("CARGO_MANIFEST_DIR")),
                    &Login::default(),
                    (24, 80),
                    1,
                    out,
                )
                .unwrap();
            Child {
                wire,
                updates,
                seen: String::new(),
            }
        }

        pub(crate) fn until(&mut self, text: &str) {
            while !self.seen.contains(text) {
                match self.updates.recv_timeout(WAIT) {
                    Ok(Update::TermOutput(_, bytes)) => {
                        self.seen.push_str(&String::from_utf8_lossy(&bytes))
                    }
                    other => panic!("{other:?} after {:?}", self.seen),
                }
            }
        }

        pub(crate) fn write(&mut self, bytes: &[u8]) {
            self.wire.write(bytes);
        }

        pub(crate) fn end(mut self) -> (String, i32) {
            loop {
                match self.updates.recv_timeout(WAIT) {
                    Ok(Update::TermOutput(_, bytes)) => {
                        self.seen.push_str(&String::from_utf8_lossy(&bytes))
                    }
                    Ok(Update::TermEnded(_, code)) => return (mem::take(&mut self.seen), code),
                    other => panic!("{other:?} after {:?}", self.seen),
                }
            }
        }
    }

    type Modes = [libc::tcflag_t; 4];

    fn modes() -> Modes {
        let mut termios: libc::termios = unsafe { mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut termios) },
            0
        );
        [
            termios.c_iflag,
            termios.c_oflag,
            termios.c_cflag,
            // The kernel sets PENDIN by itself when canonical mode comes back.
            termios.c_lflag & !libc::PENDIN,
        ]
    }

    struct Script {
        program: &'static str,
        args: Vec<OsString>,
        aside: Mutex<Option<Modes>>,
    }

    impl Asker for Script {
        fn ask(&self) -> io::Result<i32> {
            *self.aside.lock().unwrap() = Some(modes());
            RealRunner.run(self.program, &self.args)
        }
    }

    fn aside_here(program: &'static str, args: &[&str]) {
        let script = Script {
            program,
            args: args.iter().map(OsString::from).collect(),
            aside: Mutex::new(None),
        };
        let lines = LINES.map(String::from);
        let before = modes();
        let mut screen = Screen::open(io::stdout(), raw_mode).unwrap();
        let asked = screen.ask(&lines, &script);
        let back = modes()[3];
        let keys = event::poll(Duration::from_millis(200)).unwrap();
        drop(screen);
        let same = *script.aside.lock().unwrap() == Some(before);
        let raw = back & (libc::ICANON | libc::ECHO | libc::ISIG) == 0;
        println!("nobs-t-back asked={asked:?} same={same} raw={raw} keys={keys}");
    }

    fn checked_here() {
        aside_here("/bin/sh", &["-c", CHECK]);
    }

    fn answered(test: &str, typed: &[u8]) -> String {
        let mut child = Child::start(test);
        child.until("Password:");
        child.write(typed);
        let (seen, code) = child.end();
        assert_eq!(code, 0, "{seen}");
        seen
    }

    #[test]
    fn a_good_word_comes_after_the_lines_of_nobs_and_the_terminal_is_raw_again() {
        if is_child() {
            return checked_here();
        }

        let seen = answered(
            "aside::tests::a_good_word_comes_after_the_lines_of_nobs_and_the_terminal_is_raw_again",
            b"nobs-t-not-a-secret\n",
        );

        let at = |text: &str| {
            seen.find(text)
                .unwrap_or_else(|| panic!("{text} in {seen}"))
        };
        assert!(at(LINES[0]) < at(LINES[1]), "{seen}");
        assert!(at(LINES[1]) < at("Password:"), "{seen}");
        assert!(
            seen.contains("nobs-t-back asked=Ok(Ok(0)) same=true raw=true keys=false"),
            "{seen}"
        );
    }

    #[test]
    fn ctrl_c_and_ctrl_backslash_at_the_prompt_end_the_question_and_leave_the_process_alive() {
        if is_child() {
            return checked_here();
        }

        for (typed, code) in [(b"\x03", 130), (b"\x1c", 131)] {
            let seen = answered(
                "aside::tests::ctrl_c_and_ctrl_backslash_at_the_prompt_end_the_question_and_leave_the_process_alive",
                typed,
            );

            assert!(
                seen.contains(&format!(
                    "nobs-t-back asked=Ok(Ok({code})) same=true raw=true keys=false"
                )),
                "{seen}"
            );
        }
    }

    #[test]
    fn a_program_that_cannot_start_gives_its_error_and_the_terminal_is_raw_again() {
        if is_child() {
            return aside_here("/nonexistent/nobs-t-missing", &[]);
        }

        let (seen, code) = Child::start(
            "aside::tests::a_program_that_cannot_start_gives_its_error_and_the_terminal_is_raw_again",
        )
        .end();

        assert_eq!(code, 0, "{seen}");
        assert!(seen.contains("nobs-t-back asked=Ok(Err("), "{seen}");
        assert!(seen.contains("same=true raw=true keys=false"), "{seen}");
    }

    #[test]
    fn a_key_typed_while_aside_never_reaches_the_screen() {
        if is_child() {
            return checked_here();
        }

        let seen = answered(
            "aside::tests::a_key_typed_while_aside_never_reaches_the_screen",
            b"nobs-t-not-a-secret\nxyz",
        );

        assert!(
            seen.contains("nobs-t-back asked=Ok(Ok(0)) same=true raw=true keys=false"),
            "{seen}"
        );
    }
}
