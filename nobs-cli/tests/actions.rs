use std::ffi::OsString;
use std::path::PathBuf;

use clap::{CommandFactory, Parser};
use nobs_cli::{Cli, Command, WorkshopCli};
use nobs_core::locate::{locate, Kind, Located};
use nobs_core::sot::Device;
use nobs_core::status::Urls;
use nobs_tui::actions::{arguments, Action, Row, Target, ACTIONS};
use nobs_tui::shell::stacks;
use nobs_tui::{Shell, SondaTarget, Stack, StackKind, Start, Update, Workbench};

const DEVICE: &str = "srl1";
const CHAPTER: &str = "ch1";
const SUDO_NO_PROMPT: &str = "sudo-no-prompt";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(name)
}

fn model(lab: Located, stacks: Vec<Stack>) -> Shell {
    Shell::new(Start {
        lab: Some(lab),
        stacks,
        sonda: SondaTarget {
            server: None,
            api_key: None,
        },
        home: Workbench::Services,
        chooser: false,
        source: None,
        source_error: None,
        urls: Urls::defaults(),
        program: PathBuf::from("nobs"),
    })
}

fn workshop_model() -> Shell {
    let dir = fixture("workshop-one");
    let located = locate(&dir, Some(&dir)).unwrap();
    let Kind::Workshop(workshop) = &located.kind else {
        panic!("{} is not a workshop", dir.display());
    };
    let workshops = vec![(workshop.name.clone(), workshop.title.clone(), dir.clone())];
    let stacks = stacks(Some(&located), workshops).unwrap();
    let mut model = model(located, stacks);
    model.apply(Update::Devices(Ok(vec![Device {
        name: DEVICE.to_string(),
        kind: None,
        site: None,
        role: None,
        asn: None,
        maintenance: false,
        interfaces: Vec::new(),
        sessions: Vec::new(),
    }])));
    model
}

fn book_model() -> Shell {
    let dir = fixture("book-lab");
    let located = locate(&dir, Some(&dir)).unwrap();
    let stacks = stacks(Some(&located), Vec::new()).unwrap();
    model(located, stacks)
}

fn stack_index(model: &Shell, kind: StackKind, name: Option<&str>) -> usize {
    model
        .stacks()
        .iter()
        .position(|stack| stack.kind == kind && name.is_none_or(|name| stack.name == name))
        .unwrap_or_else(|| panic!("no {kind:?} stack {name:?}"))
}

fn command_line(action: &Action, workshop: &Shell, book: &Shell) -> Vec<OsString> {
    let (model, target) = match action.row {
        Row::Stack => (
            workshop,
            Target::Stack(stack_index(workshop, StackKind::Workshop, None)),
        ),
        Row::Device => (workshop, Target::Device(DEVICE.to_string())),
        Row::Chapter => (
            book,
            Target::Chapter(stack_index(book, StackKind::BookLab, Some(CHAPTER))),
        ),
        Row::Lab => (book, Target::Lab),
    };
    arguments(action, &target, model)
        .unwrap_or_else(|| panic!("{} on {:?} builds no command", action.key, action.row))
}

fn workshop_row(row: Row) -> bool {
    match row {
        Row::Stack | Row::Device => true,
        Row::Chapter | Row::Lab => false,
    }
}

fn takes_sudo_no_prompt(action: &Action) -> bool {
    let root = if workshop_row(action.row) {
        WorkshopCli::command()
    } else {
        Cli::command()
    };
    let command = action.words.iter().fold(&root, |command, word| {
        command
            .find_subcommand(word)
            .unwrap_or_else(|| panic!("{} names no subcommand {word}", action.key))
    });
    let takes = command
        .get_arguments()
        .any(|argument| argument.get_long() == Some(SUDO_NO_PROMPT));
    takes
}

#[test]
fn every_action_builds_a_command_line_that_nobs_itself_accepts() {
    let (workshop, book) = (workshop_model(), book_model());
    for action in &ACTIONS {
        let args = command_line(action, &workshop, &book);
        let cli = Cli::try_parse_from([OsString::from("nobs")].iter().chain(&args))
            .unwrap_or_else(|error| panic!("{} {args:?}: {error}", action.key));
        if workshop_row(action.row) {
            let Some(Command::External(words)) = cli.command else {
                panic!("{} {args:?} is not a workshop command", action.key);
            };
            WorkshopCli::try_parse_from(&words[1..])
                .unwrap_or_else(|error| panic!("{} {args:?}: {error}", action.key));
        }
    }
}

#[test]
fn every_action_passes_sudo_no_prompt_exactly_when_its_command_takes_it() {
    let (workshop, book) = (workshop_model(), book_model());
    let flag = OsString::from(format!("--{SUDO_NO_PROMPT}"));
    for action in &ACTIONS {
        let args = command_line(action, &workshop, &book);
        let given = args.iter().filter(|arg| **arg == flag).count();
        let expected = usize::from(takes_sudo_no_prompt(action));
        assert_eq!(
            given, expected,
            "{} {args:?} passes --{SUDO_NO_PROMPT} {given} times",
            action.key
        );
    }
}
