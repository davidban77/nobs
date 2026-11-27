use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use nobs_tui::actions::ACTIONS;
use nobs_tui::run::Invocation;
use nobs_tui::shell::Login;
use nobs_tui::{End, Launcher, Processes, Update};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("screen-and-command ");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn through_the_screen(args: &[OsString]) -> (End, Vec<String>) {
    let (out, updates) = mpsc::channel();
    let _running = Processes
        .start(
            Invocation {
                program: Path::new(env!("CARGO_BIN_EXE_nobs")),
                args,
                login: &Login::default(),
            },
            out,
        )
        .unwrap();
    let mut lines = Vec::new();
    loop {
        match updates.recv_timeout(Duration::from_secs(10)).unwrap() {
            Update::ActionLine(line) | Update::ActionError(line) => lines.push(line),
            Update::ActionEnded(end) => return (end, lines),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn an_action_ends_on_the_screen_with_the_exit_code_and_last_line_of_its_command() {
    let here = Scratch::new();
    let render = ACTIONS
        .iter()
        .find(|action| action.words == ["project", "render"])
        .unwrap();
    let args: Vec<OsString> = [OsString::from("--dir"), here.0.clone().into_os_string()]
        .into_iter()
        .chain(render.words.iter().map(OsString::from))
        .collect();

    let command = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(&args)
        .output()
        .unwrap();
    let (end, lines) = through_the_screen(&args);

    assert!(command.stdout.is_empty(), "{command:?}");
    let printed = String::from_utf8_lossy(&command.stderr);
    let last = printed.lines().last().unwrap();
    assert!(last.ends_with(' '), "{last:?}");
    let code = command.status.code().unwrap();
    assert_ne!(code, 0);
    assert_eq!(end, End::Exited(code));
    assert_eq!(lines.last().map(String::as_str), Some(last));
}
