use std::ffi::OsString;

use nobs_core::locate::Kind;

use crate::shell::{Model, StackKind, Workbench};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Stack,
    Device,
    Chapter,
    Lab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    No,
    Yes,
    Saying(&'static str),
}

#[derive(Debug)]
pub struct Action {
    pub key: char,
    pub workbench: Workbench,
    pub row: Row,
    pub words: &'static [&'static str],
    pub hint: &'static str,
    pub ask: Ask,
    pub sudo: bool,
}

const OTHER_CHAPTERS_DOWN: &str = "takes the other chapters' services down";

pub const ACTIONS: [Action; 14] = [
    Action {
        key: 'u',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["up"],
        hint: "up",
        ask: Ask::Yes,
        sudo: false,
    },
    Action {
        key: 'd',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["down"],
        hint: "down",
        ask: Ask::Yes,
        sudo: false,
    },
    Action {
        key: 'X',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["destroy"],
        hint: "destroy",
        ask: Ask::Saying("removes the stack's volumes"),
        sudo: false,
    },
    Action {
        key: 'R',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["reset"],
        hint: "reset",
        ask: Ask::Saying("clears silences, maintenance and cascade scenarios"),
        sudo: false,
    },
    Action {
        key: 'p',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["preflight"],
        hint: "preflight",
        ask: Ask::Saying("flaps srl1, leaves it in maintenance and can take 13 minutes"),
        sudo: false,
    },
    Action {
        key: 's',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["status"],
        hint: "status",
        ask: Ask::No,
        sudo: false,
    },
    Action {
        key: 'm',
        workbench: Workbench::Devices,
        row: Row::Device,
        words: &["maintenance"],
        hint: "maintenance",
        ask: Ask::Yes,
        sudo: false,
    },
    Action {
        key: 'D',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "deploy"],
        hint: "deploy",
        ask: Ask::Saying(OTHER_CHAPTERS_DOWN),
        sudo: true,
    },
    Action {
        key: 'X',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "destroy"],
        hint: "destroy",
        ask: Ask::Saying("removes the chapter's volumes, every node and the other chapters' services"),
        sudo: true,
    },
    Action {
        key: 'P',
        workbench: Workbench::Lab,
        row: Row::Lab,
        words: &["lab", "purge"],
        hint: "purge",
        ask: Ask::Saying("takes every scenario down and removes their volumes"),
        sudo: true,
    },
    Action {
        key: 'R',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "prepare"],
        hint: "prepare",
        ask: Ask::Saying("takes every scenario down first"),
        sudo: true,
    },
    Action {
        key: 'U',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "update"],
        hint: "update",
        ask: Ask::Saying(
            "recreates every service of the chapter and its unnamed volumes, and takes the other chapters' services down",
        ),
        sudo: false,
    },
    Action {
        key: 'B',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "rebuild"],
        hint: "rebuild",
        ask: Ask::Saying(OTHER_CHAPTERS_DOWN),
        sudo: false,
    },
    Action {
        key: 'S',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "show"],
        hint: "show",
        ask: Ask::No,
        sudo: true,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Stack(usize),
    Device(String),
    Chapter(usize),
    Lab,
}

fn command_words(action: &Action) -> impl Iterator<Item = OsString> {
    action
        .words
        .iter()
        .copied()
        .chain(action.sudo.then_some("--sudo-no-prompt"))
        .map(OsString::from)
}

pub fn arguments(action: &Action, target: &Target, model: &Model) -> Option<Vec<OsString>> {
    let words = command_words(action);
    let (dir, args): (OsString, Vec<OsString>) = match (action.row, target) {
        (Row::Stack, Target::Stack(index)) => {
            let stack = model
                .stacks()
                .get(*index)
                .filter(|stack| stack.kind == StackKind::Workshop)?;
            let name = OsString::from(&stack.name);
            (
                stack.path.clone().into(),
                [name].into_iter().chain(words).collect(),
            )
        }
        (Row::Device, Target::Device(name)) => {
            let lab = model.located()?;
            let Kind::Workshop(workshop) = &lab.kind else {
                return None;
            };
            let device = model.devices().iter().find(|device| device.name == *name)?;
            let flag = if device.maintenance {
                "--clear"
            } else {
                "--state"
            };
            let args = [OsString::from(&workshop.name)]
                .into_iter()
                .chain(words)
                .chain(["-d", name.as_str(), flag].map(OsString::from))
                .collect();
            (lab.dir.clone().into(), args)
        }
        (Row::Chapter, Target::Chapter(index)) => {
            let stack = model
                .stacks()
                .get(*index)
                .filter(|stack| stack.kind == StackKind::BookLab)?;
            let args = words
                .chain(["-s", stack.name.as_str()].map(OsString::from))
                .collect();
            (stack.path.clone().into(), args)
        }
        (Row::Lab, Target::Lab) => {
            let lab = model.located().filter(|_| model.book_lab())?;
            (lab.dir.clone().into(), words.collect())
        }
        _ => return None,
    };
    Some(
        [OsString::from("--dir"), dir]
            .into_iter()
            .chain(args)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::*;

    const GLOBAL_KEYS: [KeyCode; 10] = [
        KeyCode::Char('q'),
        KeyCode::Char('1'),
        KeyCode::Char('2'),
        KeyCode::Char('3'),
        KeyCode::Char('?'),
        KeyCode::Char('x'),
        KeyCode::Char('o'),
        KeyCode::Char('r'),
        KeyCode::Tab,
        KeyCode::Esc,
    ];

    fn fixed_keys(workbench: Workbench) -> Vec<KeyCode> {
        let own: &[KeyCode] = match workbench {
            Workbench::Services => &[
                KeyCode::Char('['),
                KeyCode::Char(']'),
                KeyCode::Char('j'),
                KeyCode::Char('k'),
                KeyCode::Up,
                KeyCode::Down,
                KeyCode::Char('/'),
                KeyCode::Enter,
                KeyCode::Char('l'),
                KeyCode::Char('f'),
            ],
            Workbench::Devices => &[
                KeyCode::Char('['),
                KeyCode::Char(']'),
                KeyCode::Char('j'),
                KeyCode::Char('k'),
                KeyCode::Up,
                KeyCode::Down,
                KeyCode::Char('/'),
                KeyCode::Enter,
                KeyCode::Char('-'),
                KeyCode::Char('+'),
                KeyCode::Char('f'),
            ],
            Workbench::Lab => &[
                KeyCode::Char('j'),
                KeyCode::Char('k'),
                KeyCode::Up,
                KeyCode::Down,
            ],
        };
        GLOBAL_KEYS.iter().chain(own).copied().collect()
    }

    #[test]
    fn each_workbench_has_unique_action_keys_that_no_fixed_key_of_it_uses() {
        for workbench in [Workbench::Services, Workbench::Devices, Workbench::Lab] {
            let fixed = fixed_keys(workbench);
            let mut seen = Vec::new();
            for action in ACTIONS
                .iter()
                .filter(|action| action.workbench == workbench)
            {
                assert!(
                    !seen.contains(&action.key),
                    "{workbench:?} has two actions on {}",
                    action.key
                );
                assert!(
                    !fixed.contains(&KeyCode::Char(action.key)),
                    "{workbench:?} reads {} as a fixed key",
                    action.key
                );
                seen.push(action.key);
            }
        }
    }
}
