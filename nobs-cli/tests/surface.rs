use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use clap::CommandFactory;
use nobs_cli::{Cli, WorkshopCli};

const SURFACE: &str = include_str!("surface.txt");

#[test]
fn every_command_and_flag_of_both_trees_is_in_surface_txt_in_tree_order() {
    let actual = surface();
    if actual == SURFACE {
        return;
    }
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("surface.txt");
    std::fs::write(&path, &actual).unwrap();
    panic!(
        "the command tree differs from tests/surface.txt; the tree of the code is in {}\nonly in tests/surface.txt:\n{}\nonly in the code:\n{}",
        path.display(),
        lines_only_in(SURFACE, &actual).join("\n"),
        lines_only_in(&actual, SURFACE).join("\n"),
    );
}

fn surface() -> String {
    let mut lines = Vec::new();
    for (path, mut root) in [
        ("nobs", Cli::command()),
        ("nobs <workshop>", WorkshopCli::command()),
    ] {
        root.build();
        walk(&root, path, &mut lines);
    }
    lines.join("\n") + "\n"
}

fn walk(command: &clap::Command, path: &str, lines: &mut Vec<String>) {
    lines.push(path.to_string());
    lines.extend(
        command
            .get_all_aliases()
            .map(|alias| format!("{path} alias {alias}")),
    );
    if command.is_hide_set() {
        lines.push(format!("{path} hidden"));
    }
    lines.extend(
        command
            .get_arguments()
            .map(|argument| format!("{path} {}", names(argument))),
    );
    for sub in command.get_subcommands() {
        walk(sub, &format!("{path} {}", sub.get_name()), lines);
    }
}

fn names(argument: &clap::Arg) -> String {
    let mut names: Vec<String> = argument
        .get_long()
        .into_iter()
        .map(|long| format!("--{long}"))
        .collect();
    names.extend(
        argument
            .get_all_aliases()
            .into_iter()
            .flatten()
            .map(|alias| format!("--{alias}")),
    );
    names.extend(argument.get_short().map(|short| format!("-{short}")));
    names.extend(
        argument
            .get_all_short_aliases()
            .into_iter()
            .flatten()
            .map(|short| format!("-{short}")),
    );
    if names.is_empty() {
        names.push(format!("<{}>", argument.get_id()));
    }
    if argument.is_hide_set() {
        names.push("hidden".to_string());
    }
    names.join(" ")
}

fn lines_only_in<'a>(text: &'a str, other: &str) -> Vec<&'a str> {
    let other: HashSet<&str> = other.lines().collect();
    text.lines().filter(|line| !other.contains(line)).collect()
}

#[test]
fn the_rm_help_never_says_netobs() {
    let help = help(&["docker", "rm"]);
    assert!(!help.contains("netobs"), "{help}");
}

#[test]
fn the_device_help_pages_give_their_about_texts_and_value_words() {
    let root = help(&[]);
    assert!(
        root.contains(
            "Commands for the devices of a lab: fetch, run, shell, render, push, flap and check"
        ),
        "{root}"
    );

    let group = help(&["device"]);
    for words in [
        "Print the running configuration of a device, one set line per setting",
        "Run one command on a device and print its answer",
        "Open a shell on a device in this terminal, until the shell ends",
        "Print the intended configuration of a device, from the lab's template",
    ] {
        assert!(group.contains(words), "{group}");
    }

    let run = help(&["device", "run"]);
    for words in [
        "<NAME>     Node name in the lab's topology file",
        "<COMMAND>  The command, as one quoted text",
    ] {
        assert!(run.contains(words), "{run}");
    }

    let shell = help(&["device", "shell"]);
    assert!(
        shell.contains("<NAME>  Node name in the lab's topology file"),
        "{shell}"
    );
}

#[test]
fn the_device_check_help_gives_its_about_and_the_save_flag_words() {
    let group = help(&["device"]);
    assert!(
        group.contains(
            "check   Fetch every device in turn and say which ones lack intended lines; ends with 1 when a device differs or fails"
        ),
        "{group}"
    );

    let check = help(&["device", "check"]);
    assert!(
        check.contains(
            "--save <FOLDER>  Also write each configuration nobs fetches, or why the fetch failed, to a file in this folder; the folder must not exist"
        ),
        "{check}"
    );
}

#[test]
fn the_destroy_help_never_says_deploy() {
    let help = help(&["containerlab", "destroy"]);
    assert!(!help.contains("Deploy"), "{help}");
}

#[test]
fn no_containerlab_help_page_says_show_netobs_or_labcli() {
    let pages: [&[&str]; 5] = [
        &[],
        &["containerlab"],
        &["containerlab", "deploy"],
        &["containerlab", "destroy"],
        &["containerlab", "inspect"],
    ];

    for page in pages {
        let help = help(page);
        for word in ["show", "netobs", "labcli"] {
            assert!(!help.contains(word), "{page:?}: {word}\n{help}");
        }
    }
}

#[test]
fn the_root_help_gives_the_about_texts_of_setup_and_preflight() {
    let help = help(&[]);
    assert!(
        help.contains("Check that this machine can run a workshop"),
        "{help}"
    );
    assert!(
        help.contains(
            "Check this machine, then create the .env file of every workshop nobs can find"
        ),
        "{help}"
    );
}

fn help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(args)
        .arg("--help")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{args:?}");
    String::from_utf8(output.stdout).unwrap()
}
