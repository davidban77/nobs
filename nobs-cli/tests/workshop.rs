use std::ffi::OsString;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::Parser;
use nobs_cli::{run, Cli};
use nobs_core::runner::{Call, FakeRunner};

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(relative)
}

fn enter(dir: &Path) -> PathBuf {
    std::env::remove_var("NOBS_DIR");
    std::env::set_current_dir(dir).unwrap();
    std::env::current_dir().unwrap()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-workshop-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Bare(PathBuf);

impl Bare {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("nobs-cli-workshop-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Bare {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn workshop_copy(name: &str) -> (Scratch, PathBuf) {
    let scratch = Scratch::new(name);
    let dir = scratch.0.join("nobs-t-one");
    copy_tree(&fixture("workshop-one"), &dir);
    (scratch, dir)
}

fn repo_copy(name: &str) -> (Scratch, PathBuf) {
    let scratch = Scratch::new(name);
    let repo = scratch.0.join("repo");
    copy_tree(&fixture("workshops-repo"), &repo);
    copy_tree(&fixture("workshop-one"), &repo.join("workshops/nobs-t-one"));
    (scratch, repo)
}

fn nobs_with(runner: FakeRunner, args: &[&str]) -> (i32, FakeRunner) {
    let mut line = vec!["nobs"];
    line.extend(args);
    let code = run(Cli::try_parse_from(line).unwrap(), &runner);
    (code, runner)
}

fn nobs(args: &[&str]) -> (i32, FakeRunner) {
    nobs_with(FakeRunner::returning(0), args)
}

fn compose(file: &Path, action: &[&str]) -> Vec<OsString> {
    let mut list: Vec<OsString> = ["compose", "--project-name", "nobs-t-one", "-f"]
        .iter()
        .map(OsString::from)
        .collect();
    list.push(file.as_os_str().to_owned());
    list.extend(action.iter().map(OsString::from));
    list
}

#[test]
fn every_workshop_command_makes_its_compose_calls() {
    let (_scratch, dir) = workshop_copy("lists");
    let here = enter(&dir);
    let file = here.join("docker-compose.yml");
    let no_terminal: &[&str] = if std::io::stdin().is_terminal() {
        &[]
    } else {
        &["-T"]
    };
    let exec_id = [&["exec"][..], no_terminal, &["grafana", "id"]].concat();
    let exec_sh = [&["exec"][..], no_terminal, &["grafana", "sh"]].concat();
    let cases: Vec<(Vec<&str>, Vec<Vec<&str>>)> = vec![
        (vec!["up"], vec![vec!["up", "-d"]]),
        (vec!["up", "--build"], vec![vec!["up", "-d", "--build"]]),
        (vec!["up", "grafana"], vec![vec!["up", "-d", "grafana"]]),
        (vec!["down"], vec![vec!["down"]]),
        (
            vec!["destroy"],
            vec![vec!["down", "--volumes", "--remove-orphans"]],
        ),
        (vec!["restart", "grafana"], vec![vec!["restart", "grafana"]]),
        (
            vec!["restart"],
            vec![vec!["down"], vec!["up", "-d", "--build"]],
        ),
        (vec!["logs"], vec![vec!["logs", "-f", "--tail=200"]]),
        (
            vec!["logs", "--tail", "5", "--no-follow", "grafana"],
            vec![vec!["logs", "--tail=5", "grafana"]],
        ),
        (
            vec!["logs", "--no-follow", "-f"],
            vec![vec!["logs", "-f", "--tail=200"]],
        ),
        (
            vec!["logs", "-f", "--no-follow"],
            vec![vec!["logs", "--tail=200"]],
        ),
        (vec!["exec", "grafana", "id"], vec![exec_id]),
        (vec!["exec", "grafana"], vec![exec_sh]),
        (vec!["build"], vec![vec!["build"], vec!["up", "-d"]]),
        (
            vec!["build", "webhook"],
            vec![vec!["build", "webhook"], vec!["up", "-d", "webhook"]],
        ),
    ];

    for (args, expected) in cases {
        let mut line = vec!["nobs-t-one"];
        line.extend(&args);

        let (code, runner) = nobs(&line);

        assert_eq!(code, 0, "{args:?}");
        let calls = runner.calls();
        assert!(
            calls.iter().all(|call| call.program == "docker"),
            "{args:?}"
        );
        let made: Vec<&[OsString]> = calls.iter().map(|call| call.args.as_slice()).collect();
        let expected: Vec<Vec<OsString>> = expected
            .iter()
            .map(|action| compose(&file, action))
            .collect();
        assert_eq!(made, expected, "{args:?}");
    }
}

#[test]
fn restart_with_no_services_stops_after_a_failing_down() {
    let (_scratch, dir) = workshop_copy("restart-fails");
    let here = enter(&dir);

    let (code, runner) = nobs_with(FakeRunner::returning(1), &["nobs-t-one", "restart"]);

    assert_eq!(code, 1);
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        calls[0].args,
        compose(&here.join("docker-compose.yml"), &["down"])
    );
}

#[test]
fn build_stops_after_a_failing_build() {
    let (_scratch, dir) = workshop_copy("build-fails");
    let here = enter(&dir);

    let (code, runner) = nobs_with(FakeRunner::returning(1), &["nobs-t-one", "build"]);

    assert_eq!(code, 1);
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        calls[0].args,
        compose(&here.join("docker-compose.yml"), &["build"])
    );
}

#[test]
fn up_exits_with_the_code_of_docker() {
    let (_scratch, dir) = workshop_copy("up-code");
    enter(&dir);

    let (code, runner) = nobs_with(FakeRunner::returning(3), &["nobs-t-one", "up"]);

    assert_eq!(code, 3);
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn setup_makes_no_call_and_copies_the_example() {
    let (_scratch, dir) = workshop_copy("setup");
    enter(&dir);

    let (code, runner) = nobs(&["nobs-t-one", "setup"]);

    assert_eq!(code, 0);
    assert!(runner.calls().is_empty());
    assert!(runner.outputs().is_empty());
    assert_eq!(
        fs::read(dir.join(".env")).unwrap(),
        fs::read(fixture("workshop-one/.env.example")).unwrap()
    );
}

#[test]
fn up_with_an_example_that_cannot_be_copied_exits_1_without_a_call() {
    let (_scratch, dir) = workshop_copy("up-bad-example");
    fs::remove_file(dir.join(".env.example")).unwrap();
    fs::create_dir(dir.join(".env.example")).unwrap();
    enter(&dir);

    let (code, runner) = nobs(&["nobs-t-one", "up"]);

    assert_eq!(code, 1);
    assert!(runner.calls().is_empty());
}

#[test]
fn a_foreign_stack_stops_up_build_and_restart_before_anything_else() {
    let (_scratch, repo) = repo_copy("guard-stops");
    enter(&repo);
    let asked = Call {
        program: "docker".to_string(),
        env: Vec::new(),
        args: [
            "ps",
            "-a",
            "--filter",
            "label=com.docker.compose.project=nobs-t-two",
            "--format",
            "{{.Names}}",
        ]
        .iter()
        .map(OsString::from)
        .collect(),
    };

    for command in [
        &["up"][..],
        &["build"][..],
        &["restart"][..],
        &["restart", "grafana"][..],
    ] {
        let mut line = vec!["nobs-t-one"];
        line.extend(command);

        let (code, runner) = nobs_with(FakeRunner::answering("sonda-server\n"), &line);

        assert_eq!(code, 1, "{command:?}");
        assert!(runner.calls().is_empty(), "{command:?}");
        assert_eq!(runner.outputs(), vec![asked.clone()], "{command:?}");
        assert!(
            !repo.join("workshops/nobs-t-one/.env").exists(),
            "{command:?}"
        );
    }
}

#[test]
fn a_guard_question_docker_answers_with_an_error_code_counts_as_no_containers() {
    let (_scratch, repo) = repo_copy("guard-code");
    enter(&repo);

    let (code, runner) = nobs_with(FakeRunner::returning(3), &["nobs-t-one", "up"]);

    assert_eq!(code, 3);
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        calls[0].args,
        compose(
            &repo.join("workshops/nobs-t-one/docker-compose.yml"),
            &["up", "-d"]
        )
    );
}

#[test]
fn a_docker_that_cannot_start_for_the_guard_stops_up_with_exit_1() {
    let (_scratch, repo) = repo_copy("guard-cannot-run");
    enter(&repo);

    let (code, runner) = nobs_with(FakeRunner::failing(), &["nobs-t-one", "up"]);

    assert_eq!(code, 1);
    assert!(runner.calls().is_empty());
}

#[test]
fn down_does_not_ask_about_foreign_stacks() {
    let (_scratch, repo) = repo_copy("guard-down");
    enter(&repo);

    let (code, runner) = nobs_with(
        FakeRunner::answering("sonda-server\n"),
        &["nobs-t-one", "down"],
    );

    assert_eq!(code, 0);
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        calls[0].args,
        compose(
            &repo.join("workshops/nobs-t-one/docker-compose.yml"),
            &["down"]
        )
    );
    assert!(runner.outputs().is_empty());
}

#[test]
fn a_bare_command_inside_a_workshop_is_that_workshops_command() {
    let (_scratch, dir) = workshop_copy("bare-rule");
    let here = enter(&dir);
    let expected = compose(&here.join("docker-compose.yml"), &["up", "-d"]);

    let (code, named) = nobs(&["nobs-t-one", "up"]);
    assert_eq!(code, 0);
    assert_eq!(named.calls()[0].args, expected);

    for folder in [here.clone(), here.join("sub")] {
        enter(&folder);

        let (code, runner) = nobs(&["up"]);

        assert_eq!(code, 0, "{}", folder.display());
        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "{}", folder.display());
        assert_eq!(calls[0].args, expected, "{}", folder.display());
    }
}

#[test]
fn workshops_inside_a_workshop_is_still_the_root_command() {
    let (_scratch, dir) = workshop_copy("bare-workshops");
    enter(&dir);

    let (code, runner) = nobs(&["workshops"]);

    assert_eq!(code, 0);
    assert!(runner.calls().is_empty());
    assert!(runner.outputs().is_empty());
}

#[test]
fn a_bare_command_outside_a_workshop_exits_2_without_a_call() {
    let bare = Bare::new("bare-outside");
    enter(&bare.0);

    let (code, runner) = nobs(&["up"]);

    assert_eq!(code, 2);
    assert!(runner.calls().is_empty());
    assert!(runner.outputs().is_empty());
}
