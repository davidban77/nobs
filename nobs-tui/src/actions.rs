use std::ffi::OsString;

use nobs_core::locate::{Kind, Nodes};

use crate::shell::{Model, StackKind, Workbench};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Stack,
    Device,
    Node,
    Chapter,
    Lab(&'static [Nodes]),
}

impl Row {
    pub(crate) fn needs(self) -> Option<&'static [Nodes]> {
        match self {
            Row::Chapter => Some(&[Nodes::Chapters]),
            Row::Lab(nodes) => Some(nodes),
            Row::Stack | Row::Device | Row::Node => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    No,
    Yes,
    Saying(&'static str),
    Text,
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
    pub keeps: bool,
    pub login: bool,
}

const OTHER_CHAPTERS_DOWN: &str = "takes the other chapters' services down";
const WITH_NODES: &[Nodes] = &[Nodes::Project, Nodes::Topology];

pub const ACTIONS: [Action; 21] = [
    Action {
        key: 'u',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["up"],
        hint: "up",
        ask: Ask::Yes,
        sudo: false,
        keeps: false,
        login: false,
    },
    Action {
        key: 'd',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["down"],
        hint: "down",
        ask: Ask::Yes,
        sudo: false,
        keeps: false,
        login: false,
    },
    Action {
        key: 'X',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["destroy"],
        hint: "destroy",
        ask: Ask::Saying("removes the stack's volumes"),
        sudo: false,
        keeps: false,
        login: false,
    },
    Action {
        key: 'R',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["reset"],
        hint: "reset",
        ask: Ask::Saying("clears silences, maintenance and cascade scenarios"),
        sudo: false,
        keeps: false,
        login: false,
    },
    Action {
        key: 'p',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["preflight"],
        hint: "preflight",
        ask: Ask::Saying("flaps srl1, leaves it in maintenance and can take 13 minutes"),
        sudo: false,
        keeps: false,
        login: false,
    },
    Action {
        key: 's',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["status"],
        hint: "status",
        ask: Ask::No,
        sudo: false,
        keeps: false,
        login: false,
    },
    Action {
        key: 'm',
        workbench: Workbench::Devices,
        row: Row::Device,
        words: &["maintenance"],
        hint: "maintenance",
        ask: Ask::Yes,
        sudo: false,
        keeps: false,
        login: false,
    },
    Action {
        key: 'g',
        workbench: Workbench::Devices,
        row: Row::Node,
        words: &["device", "fetch"],
        hint: "get config",
        ask: Ask::No,
        sudo: false,
        keeps: true,
        login: true,
    },
    Action {
        key: 's',
        workbench: Workbench::Devices,
        row: Row::Node,
        words: &["device", "run"],
        hint: "run command",
        ask: Ask::Text,
        sudo: false,
        keeps: false,
        login: true,
    },
    Action {
        key: 'D',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "deploy"],
        hint: "deploy",
        ask: Ask::Saying(OTHER_CHAPTERS_DOWN),
        sudo: true,
        keeps: false,
        login: false,
    },
    Action {
        key: 'X',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "destroy"],
        hint: "destroy",
        ask: Ask::Saying("removes the chapter's volumes, every node and the other chapters' services"),
        sudo: true,
        keeps: false,
        login: false,
    },
    Action {
        key: 'P',
        workbench: Workbench::Lab,
        row: Row::Lab(&[Nodes::Chapters]),
        words: &["lab", "purge"],
        hint: "purge",
        ask: Ask::Saying("takes every scenario down and removes their volumes"),
        sudo: true,
        keeps: false,
        login: false,
    },
    Action {
        key: 'R',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "prepare"],
        hint: "prepare",
        ask: Ask::Saying("takes every scenario down first"),
        sudo: true,
        keeps: false,
        login: false,
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
        keeps: false,
        login: false,
    },
    Action {
        key: 'B',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "rebuild"],
        hint: "rebuild",
        ask: Ask::Saying(OTHER_CHAPTERS_DOWN),
        sudo: false,
        keeps: false,
        login: false,
    },
    Action {
        key: 'S',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "show"],
        hint: "show",
        ask: Ask::No,
        sudo: true,
        keeps: false,
        login: false,
    },
    Action {
        key: 'D',
        workbench: Workbench::Lab,
        row: Row::Lab(&[Nodes::Project]),
        words: &["project", "deploy"],
        hint: "deploy",
        ask: Ask::Saying("writes the files under containerlab/ first"),
        sudo: true,
        keeps: false,
        login: false,
    },
    Action {
        key: 'D',
        workbench: Workbench::Lab,
        row: Row::Lab(&[Nodes::Topology]),
        words: &["containerlab", "deploy"],
        hint: "deploy",
        ask: Ask::Yes,
        sudo: true,
        keeps: false,
        login: false,
    },
    Action {
        key: 'X',
        workbench: Workbench::Lab,
        row: Row::Lab(WITH_NODES),
        words: &["containerlab", "destroy"],
        hint: "destroy",
        ask: Ask::Saying("removes every node of the lab"),
        sudo: true,
        keeps: false,
        login: false,
    },
    Action {
        key: 'R',
        workbench: Workbench::Lab,
        row: Row::Lab(&[Nodes::Project]),
        words: &["project", "render"],
        hint: "render",
        ask: Ask::No,
        sudo: false,
        keeps: false,
        login: false,
    },
    Action {
        key: 'S',
        workbench: Workbench::Lab,
        row: Row::Lab(WITH_NODES),
        words: &["containerlab", "inspect"],
        hint: "show",
        ask: Ask::No,
        sudo: true,
        keeps: false,
        login: false,
    },
];

pub(crate) fn fits(action: &Action, model: &Model) -> bool {
    fits_nodes(action, model.nodes_from())
}

fn fits_nodes(action: &Action, nodes: Option<Nodes>) -> bool {
    action
        .row
        .needs()
        .is_none_or(|needs| nodes.is_some_and(|nodes| needs.contains(&nodes)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Stack(usize),
    Device(String),
    Chapter(usize),
    Lab,
}

fn command_words(action: &Action) -> impl Iterator<Item = Vec<OsString>> {
    action
        .words
        .iter()
        .copied()
        .chain(action.sudo.then_some("--sudo-no-prompt"))
        .map(|word| vec![OsString::from(word)])
}

fn pair(flag: &str, value: impl Into<OsString>) -> Vec<OsString> {
    vec![OsString::from(flag), value.into()]
}

pub fn arguments(action: &Action, target: &Target, model: &Model) -> Option<Vec<OsString>> {
    pieces(action, target, model).map(|pieces| flat(&pieces))
}

pub(crate) fn flat(pieces: &[Vec<OsString>]) -> Vec<OsString> {
    pieces.concat()
}

pub(crate) fn pieces(
    action: &Action,
    target: &Target,
    model: &Model,
) -> Option<Vec<Vec<OsString>>> {
    let words = command_words(action);
    let (dir, args): (OsString, Vec<Vec<OsString>>) = match (action.row, target) {
        (Row::Stack, Target::Stack(index)) => {
            let stack = model
                .stacks()
                .get(*index)
                .filter(|stack| stack.kind == StackKind::Workshop)?;
            let name = vec![OsString::from(&stack.name)];
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
            let args = [vec![OsString::from(&workshop.name)]]
                .into_iter()
                .chain(words)
                .chain([pair("-d", name), vec![OsString::from(flag)]])
                .collect();
            (lab.dir.clone().into(), args)
        }
        (Row::Node, Target::Device(name)) => {
            let lab = model.located().filter(|_| model.lab_with_nodes())?;
            let args = words.chain([vec![OsString::from(name)]]).collect();
            (lab.dir.clone().into(), args)
        }
        (Row::Chapter, Target::Chapter(index)) => {
            let stack = model
                .stacks()
                .get(*index)
                .filter(|stack| stack.kind == StackKind::BookLab)?;
            let args = words.chain([pair("-s", &stack.name)]).collect();
            (stack.path.clone().into(), args)
        }
        (Row::Lab(_), Target::Lab) => {
            let lab = model.located().filter(|_| fits(action, model))?;
            (lab.dir.clone().into(), words.collect())
        }
        _ => return None,
    };
    Some([pair("--dir", dir)].into_iter().chain(args).collect())
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::*;
    use crate::shell::WORKBENCHES;

    const GLOBAL_KEYS: [KeyCode; 7] = [
        KeyCode::Char('q'),
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
                KeyCode::Char('f'),
            ],
            Workbench::Observability => &[
                KeyCode::Char('j'),
                KeyCode::Char('k'),
                KeyCode::Up,
                KeyCode::Down,
                KeyCode::Char('/'),
                KeyCode::Enter,
            ],
        };
        let switch = ('1'..='9').take(WORKBENCHES.len()).map(KeyCode::Char);
        GLOBAL_KEYS
            .iter()
            .chain(own)
            .copied()
            .chain(switch)
            .collect()
    }

    #[test]
    fn each_workbench_has_unique_action_keys_in_each_lab_that_no_fixed_key_of_it_uses() {
        let labs = [
            None,
            Some(Nodes::Chapters),
            Some(Nodes::Project),
            Some(Nodes::Topology),
        ];
        for workbench in WORKBENCHES {
            let fixed = fixed_keys(workbench);
            for lab in labs {
                let mut seen = Vec::new();
                for action in ACTIONS
                    .iter()
                    .filter(|action| action.workbench == workbench && fits_nodes(action, lab))
                {
                    assert!(
                        !seen.contains(&action.key),
                        "{workbench:?} in {lab:?} has two actions on {}",
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
}
