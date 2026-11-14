use std::ffi::OsString;

use nobs_core::locate::Kind;

use crate::shell::{Model, StackKind, Workbench};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Stack,
    Device,
    Chapter,
}

#[derive(Debug)]
pub struct Action {
    pub key: char,
    pub workbench: Workbench,
    pub row: Row,
    pub words: &'static [&'static str],
}

pub const ACTIONS: [Action; 5] = [
    Action {
        key: 'u',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["up"],
    },
    Action {
        key: 'd',
        workbench: Workbench::Services,
        row: Row::Stack,
        words: &["down"],
    },
    Action {
        key: 'm',
        workbench: Workbench::Devices,
        row: Row::Device,
        words: &["maintenance"],
    },
    Action {
        key: 'D',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "deploy", "--sudo-no-prompt"],
    },
    Action {
        key: 'X',
        workbench: Workbench::Lab,
        row: Row::Chapter,
        words: &["lab", "destroy", "--sudo-no-prompt"],
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Stack(usize),
    Device(String),
    Chapter(usize),
}

pub fn arguments(action: &Action, target: &Target, model: &Model) -> Option<Vec<OsString>> {
    let words = action.words.iter().map(OsString::from);
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
        _ => return None,
    };
    Some(
        [OsString::from("--dir"), dir]
            .into_iter()
            .chain(args)
            .collect(),
    )
}
