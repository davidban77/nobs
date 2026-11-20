use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nobs_core::lab_vars;
use nobs_core::locate::{Kind, Located};
use nobs_core::runner::{Output, Runner};
use nobs_core::sot::Fixed;
use nobs_core::status::Urls;
use nobs_tui::{
    handle, Command, Pollers, Shell, SondaTarget, Stack, StackKind, Start, Update, Workbench,
};

use crate::stub::Stub;

const WORKSHOP_ONE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/fixtures/workshop-one/lab_vars.yml"
);

struct Silent;

impl Runner for Silent {
    fn run(&self, _program: &str, _args: &[OsString]) -> io::Result<i32> {
        Ok(0)
    }

    fn output(&self, _program: &str, _args: &[OsString]) -> io::Result<Output> {
        Ok(Output {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

pub fn start_pollers(stub: &Stub) -> (Pollers, Receiver<Update>) {
    let address = || stub.url().to_string();
    let urls = Urls {
        grafana: address(),
        prometheus: address(),
        loki: address(),
        alertmanager: address(),
        infrahub: address(),
        prefect: address(),
        sonda: address(),
        set: Vec::new(),
    };
    let text = fs::read_to_string(WORKSHOP_ONE).unwrap();
    let source = Fixed::from_lab_vars(&lab_vars::read(&text).unwrap());
    Pollers::start(
        Arc::new(Silent),
        Vec::new(),
        0,
        SondaTarget {
            server: None,
            api_key: None,
        },
        Some((Arc::new(source), None)),
        Some(urls),
        None,
    )
}

pub fn shell(home: Workbench) -> Shell {
    let path = PathBuf::from("/ws");
    Shell::new(Start {
        lab: Some(Located {
            dir: path.clone(),
            kind: Kind::LabcliLab(None),
        }),
        stacks: vec![Stack {
            name: "ws".to_string(),
            kind: StackKind::Workshop,
            compose: path.join("docker-compose.yml"),
            path,
            project: None,
            env: Vec::new(),
            shows_sonda: false,
        }],
        sonda: SondaTarget {
            server: None,
            api_key: None,
        },
        home,
        chooser: false,
        source: None,
        source_error: None,
        source_service: None,
        urls: Urls::defaults(),
        program: PathBuf::from("nobs"),
        login: Arc::new(|_: &str| None),
    })
}

pub fn send(pollers: &Pollers, commands: Vec<Command>) {
    for command in commands {
        pollers.send(command);
    }
}

pub fn pump_until(
    model: &mut Shell,
    pollers: &Pollers,
    updates: &Receiver<Update>,
    wait: Duration,
    wanted: impl Fn(&Update) -> bool,
) -> bool {
    let end = Instant::now() + wait;
    while let Ok(update) = updates.recv_timeout(end.saturating_duration_since(Instant::now())) {
        let found = wanted(&update);
        send(pollers, model.apply(update));
        if found {
            return true;
        }
    }
    false
}

pub fn press(model: &mut Shell, typed: char) -> Vec<Command> {
    handle(
        model,
        KeyEvent::new(KeyCode::Char(typed), KeyModifiers::NONE),
    )
}
