#[path = "support/listing.rs"]
mod listing;

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use clap::Parser;
use listing::listing;
use nobs_cli::{run, Cli};
use nobs_core::project::{self, Rendered};
use nobs_core::runner::{FakeRunner, Output as RunnerOutput, Runner};

const FILES: [&str; 4] = [
    "lab.clab.yml",
    "lab_vars.yml",
    "configs/nobs-t-srl1.cfg",
    "configs/nobs-t-srl2.cfg",
];

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str) -> Self {
        let copy = Self::bare(name);
        copy_tree(&fixture(), copy.dir());
        copy
    }

    fn bare(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-project-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }

    fn dir(&self) -> &Path {
        &self.0
    }

    fn shown(&self) -> &str {
        self.0.to_str().unwrap()
    }

    fn written(&self, name: &str) -> PathBuf {
        self.0.join("containerlab").join(name)
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/project-two-node")
}

fn expected(name: &str) -> String {
    fs::read_to_string(fixture().join("expected").join(name)).unwrap()
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

fn nobs(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(args)
        .current_dir(cwd)
        .env_remove("NOBS_DIR")
        .output()
        .unwrap()
}

fn nobs_in(copy: &Copy, args: &[&str]) -> Output {
    let mut all = vec!["--dir", copy.shown()];
    all.extend_from_slice(args);
    nobs(&all, copy.dir())
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn wrote_lines() -> String {
    FILES
        .iter()
        .map(|name| format!("wrote containerlab/{name}\n"))
        .collect()
}

fn would_write_lines() -> String {
    FILES
        .iter()
        .map(|name| format!("would write containerlab/{name}\n"))
        .collect()
}

struct Recording {
    folder: PathBuf,
    calls: RefCell<Vec<(String, Vec<OsString>, bool)>>,
}

impl Recording {
    fn new(folder: &Path) -> Self {
        Self {
            folder: folder.to_path_buf(),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Runner for Recording {
    fn run(&self, program: &str, args: &[OsString]) -> io::Result<i32> {
        let written = FILES
            .iter()
            .all(|name| self.folder.join("containerlab").join(name).is_file());
        self.calls
            .borrow_mut()
            .push((program.to_string(), args.to_vec(), written));
        Ok(0)
    }

    fn output(&self, _program: &str, _args: &[OsString]) -> io::Result<RunnerOutput> {
        Err(io::Error::other("deploy asks for no output"))
    }
}

fn deploy_cli(copy: &Copy, flags: &[&str]) -> Cli {
    let mut args = vec!["nobs", "--dir", copy.shown(), "project"];
    args.extend_from_slice(flags);
    args.push("deploy");
    Cli::try_parse_from(args).unwrap()
}

#[test]
fn render_writes_the_four_files_equal_to_expected_and_prints_one_line_each() {
    let copy = Copy::new("render-writes");

    let output = nobs_in(&copy, &["project", "render"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), wrote_lines());
    for name in FILES {
        let written = fs::read(copy.written(name)).unwrap();
        assert_eq!(written, expected(name).into_bytes(), "{name}");
    }
}

#[test]
fn render_check_exits_1_and_names_the_file_when_one_differs() {
    let copy = Copy::new("check-differs");

    let output = nobs_in(&copy, &["project", "render", "--check"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), would_write_lines());
    assert!(!copy.dir().join("containerlab").exists());

    assert_eq!(
        nobs_in(&copy, &["project", "render"]).status.code(),
        Some(0)
    );
    let vars = copy.written("lab_vars.yml");
    let mut text = fs::read_to_string(&vars).unwrap();
    text.push_str("extra: 1\n");
    fs::write(&vars, text).unwrap();

    let output = nobs_in(&copy, &["project", "render", "--check"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "would write containerlab/lab_vars.yml\n");
}

#[test]
fn render_check_exits_0_and_writes_nothing_when_every_file_is_current() {
    let copy = Copy::new("check-current");
    assert_eq!(
        nobs_in(&copy, &["project", "render"]).status.code(),
        Some(0)
    );
    let before: Vec<Vec<u8>> = FILES
        .iter()
        .map(|name| fs::read(copy.written(name)).unwrap())
        .collect();

    let output = nobs_in(&copy, &["project", "render", "--check"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "");
    let after: Vec<Vec<u8>> = FILES
        .iter()
        .map(|name| fs::read(copy.written(name)).unwrap())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn deploy_writes_the_files_then_runs_the_slice_4_deploy_list_with_preserve_env() {
    let copy = Copy::new("deploy-list");
    let runner = Recording::new(copy.dir());

    assert_eq!(run(deploy_cli(&copy, &[]), &runner), 0);

    let topology = copy.dir().join("containerlab/lab.clab.yml");
    let calls = runner.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "sudo");
    assert_eq!(
        calls[0].1,
        vec![
            OsString::from("--preserve-env=NOBS_T_LABEL"),
            OsString::from("containerlab"),
            OsString::from("deploy"),
            OsString::from("-t"),
            topology.into_os_string(),
        ]
    );
    assert!(
        calls[0].2,
        "the four files were not all written at call time"
    );
}

#[test]
fn deploy_returns_the_childs_exit_code() {
    let copy = Copy::new("deploy-code");
    let runner = FakeRunner::returning(3);

    assert_eq!(run(deploy_cli(&copy, &[]), &runner), 3);
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn deploy_without_sudo_runs_containerlab_itself() {
    let copy = Copy::new("deploy-no-sudo");
    let runner = FakeRunner::returning(0);

    assert_eq!(run(deploy_cli(&copy, &["--no-sudo"]), &runner), 0);

    let topology = copy.dir().join("containerlab/lab.clab.yml");
    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "containerlab");
    assert_eq!(
        calls[0].args,
        vec![
            OsString::from("deploy"),
            OsString::from("-t"),
            topology.into_os_string()
        ]
    );
}

#[test]
fn a_lab_folder_without_a_project_file_exits_1_naming_the_folder() {
    let copy = Copy::bare("no-project-file");
    fs::write(copy.dir().join("docker-compose.yml"), "").unwrap();

    let output = nobs_in(&copy, &["project", "render"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!("nobs: project: no nobs_project.yml in {}\n", copy.shown())
    );
    assert!(!copy.dir().join("containerlab").exists());
}

#[test]
fn a_kind_without_a_template_exits_1_naming_the_kind_and_writes_nothing() {
    let copy = Copy::new("no-template");
    fs::remove_file(copy.dir().join("templates/srl.j2")).unwrap();

    let output = nobs_in(&copy, &["project", "render"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: project: no template for kind nokia_srlinux: expected templates/srl.j2\n"
    );
    assert!(!copy.dir().join("containerlab").exists());
}

#[test]
fn a_project_file_that_does_not_read_exits_1_naming_the_file_and_the_line() {
    let copy = Copy::new("bad-project");
    let file = copy.dir().join("nobs_project.yml");
    let text = fs::read_to_string(&file).unwrap();
    let changed = text.replace("node_b: nobs-t-srl2", "node_b: nobs-t-srl9");
    assert_ne!(text, changed);
    fs::write(&file, changed).unwrap();

    let output = nobs_in(&copy, &["project", "render"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr(&output);
    assert!(
        stderr.starts_with("nobs: project: nobs_project.yml: line "),
        "{stderr}"
    );
    assert!(stderr.contains("nobs-t-srl9"), "{stderr}");
    assert!(!copy.dir().join("containerlab").exists());
}

#[test]
fn render_from_a_subfolder_finds_the_project_file_upward() {
    let copy = Copy::new("subfolder");
    let sub = copy.dir().join("sub");
    fs::create_dir(&sub).unwrap();

    let output = nobs(&["project", "render"], &sub);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    for name in FILES {
        assert!(copy.written(name).is_file(), "{name}");
    }
}

#[test]
fn the_project_flag_names_another_file() {
    let copy = Copy::new("project-flag");
    fs::rename(
        copy.dir().join("nobs_project.yml"),
        copy.dir().join("other.yml"),
    )
    .unwrap();
    fs::write(copy.dir().join("docker-compose.yml"), "").unwrap();

    let output = nobs_in(&copy, &["project", "--project", "other.yml", "render"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), wrote_lines());
    for name in FILES {
        assert!(copy.written(name).is_file(), "{name}");
    }

    let output = nobs_in(&copy, &["project", "--project", "missing.yml", "render"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: project: no project file at missing.yml\n"
    );
}

#[test]
fn the_project_help_lists_render_and_deploy_and_the_flags() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));

    let output = nobs(&["project", "--help"], here);

    assert_eq!(output.status.code(), Some(0));
    let help = stdout(&output);
    assert_eq!(commands_block(&help), ["render", "deploy"]);
    for flag in ["--project", "--sudo", "--no-sudo"] {
        assert!(help.contains(flag), "{flag}\n{help}");
    }

    let output = nobs(&["project", "render", "--help"], here);

    assert_eq!(output.status.code(), Some(0));
    let help = stdout(&output);
    assert!(help.contains("--check"), "{help}");
}

const PASSWORD_SUDO: &str = "#!/bin/sh\nif [ \"$1\" = \"-n\" ]; then\n  echo 'sudo: a password is required' >&2\n  exit 1\nfi\n: > \"${0%/*}/marker\"\nexit 97\n";

const MARKING: &str = "#!/bin/sh\n: > \"${0%/*}/marker\"\nexit 97\n";

fn password_bin(name: &str) -> Copy {
    use std::os::unix::fs::PermissionsExt;
    let bin = Copy::bare(name);
    for (program, text) in [("sudo", PASSWORD_SUDO), ("containerlab", MARKING)] {
        let path = bin.dir().join(program);
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn nobs_on_path(copy: &Copy, bin: &Copy, args: &[&str]) -> Output {
    let mut all = vec!["--dir", copy.shown()];
    all.extend_from_slice(args);
    Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(all)
        .current_dir(copy.dir())
        .env_clear()
        .env("PATH", bin.dir())
        .output()
        .unwrap()
}

fn refused(command: &str) -> String {
    format!(
        "sudo: a password is required\n\
         nobs: {command}: sudo needs a password and cannot ask for it here; run sudo -v in this terminal, then try again\n"
    )
}

#[test]
fn render_check_and_rendered_agree_for_each_state() {
    let copy = Copy::new("agree");
    let dir = copy.dir();

    for state in ["not rendered", "as rendered", "one differs", "error"] {
        match state {
            "as rendered" => assert_eq!(
                nobs_in(&copy, &["project", "render"]).status.code(),
                Some(0)
            ),
            "one differs" => fs::write(copy.written("lab_vars.yml"), "extra: 1\n").unwrap(),
            "error" => fs::write(dir.join("nobs_project.yml"), "main: [\n").unwrap(),
            _ => {}
        }
        let before = listing(dir);

        let rendered = project::rendered(dir);

        assert_eq!(listing(dir), before, "{state}");
        let output = nobs_in(&copy, &["project", "render", "--check"]);
        let check = (output.status.code(), stdout(&output), stderr(&output));
        let expected = match &rendered {
            Rendered::NotRendered => (Some(1), would_write_lines(), String::new()),
            Rendered::AsRendered => (Some(0), String::new(), String::new()),
            Rendered::Differ { count: 1, of: 4 } => (
                Some(1),
                "would write containerlab/lab_vars.yml\n".to_string(),
                String::new(),
            ),
            Rendered::Error(text) => (Some(1), String::new(), format!("nobs: project: {text}\n")),
            other => panic!("{state}: {other:?}"),
        };
        assert_eq!(check, expected, "{state}: {rendered:?}");
    }
}

#[test]
fn deploy_with_sudo_no_prompt_and_a_sudo_that_needs_a_password_writes_and_runs_nothing() {
    let copy = Copy::new("deploy-refused");
    let bin = password_bin("deploy-refused-bin");
    let before = listing(copy.dir());

    let output = nobs_on_path(&copy, &bin, &["project", "deploy", "--sudo-no-prompt"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(listing(copy.dir()), before);
    assert_eq!(stderr(&output), refused("project deploy"));
    assert_eq!(stdout(&output), "");
    assert!(!bin.dir().join("marker").exists());
}

#[test]
fn containerlab_commands_with_sudo_no_prompt_and_a_sudo_that_needs_a_password_run_nothing() {
    let copy = Copy::new("containerlab-refused");
    let bin = password_bin("containerlab-refused-bin");
    assert_eq!(
        nobs_in(&copy, &["project", "render"]).status.code(),
        Some(0)
    );

    for command in ["deploy", "destroy", "inspect"] {
        let output = nobs_on_path(&copy, &bin, &["containerlab", command, "--sudo-no-prompt"]);

        assert_eq!(output.status.code(), Some(1), "{command}");
        assert_eq!(
            stderr(&output),
            refused(&format!("containerlab {command}")),
            "{command}"
        );
        assert!(!bin.dir().join("marker").exists(), "{command}");
    }
}

#[test]
fn in_a_project_folder_deploy_destroy_and_inspect_name_the_rendered_topology() {
    let copy = Copy::new("one-topology");
    fs::write(
        copy.dir().join("nobs.toml"),
        "kind = \"lab\"\ntopology = \"t/x.yml\"\n",
    )
    .unwrap();
    fs::create_dir_all(copy.dir().join("t")).unwrap();
    fs::write(copy.dir().join("t/x.yml"), "name: x\n").unwrap();
    fs::create_dir_all(copy.dir().join("containerlab")).unwrap();
    fs::write(copy.written("lab.clab.yaml"), "name: hand\n").unwrap();
    let topology = |runner: &FakeRunner| {
        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        let at = calls[0].args.iter().position(|arg| arg == "-t").unwrap();
        PathBuf::from(&calls[0].args[at + 1])
    };
    let mut named = Vec::new();

    let runner = FakeRunner::returning(0);
    assert_eq!(run(deploy_cli(&copy, &[]), &runner), 0);
    named.push(topology(&runner));
    for command in ["deploy", "destroy", "inspect"] {
        let runner = FakeRunner::returning(0);
        let cli =
            Cli::try_parse_from(["nobs", "--dir", copy.shown(), "containerlab", command]).unwrap();
        assert_eq!(run(cli, &runner), 0, "{command}");
        named.push(topology(&runner));
    }

    assert_eq!(named, vec![copy.written("lab.clab.yml"); 4]);
}

#[test]
fn the_sudo_no_prompt_flag_of_containerlab_and_project_deploy_says_what_it_does() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    for command in [
        &["containerlab", "deploy", "--help"][..],
        &["containerlab", "destroy", "--help"],
        &["containerlab", "inspect", "--help"],
        &["project", "deploy", "--help"],
    ] {
        let output = nobs(command, here);

        assert_eq!(output.status.code(), Some(0), "{command:?}");
        let help = stdout(&output);
        assert!(
            help.contains(
                "--sudo-no-prompt  Fail instead of asking for a password when sudo needs one"
            ),
            "{command:?}\n{help}"
        );
    }
}

fn commands_block(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .filter(|word| *word != "help")
        .map(str::to_owned)
        .collect()
}
