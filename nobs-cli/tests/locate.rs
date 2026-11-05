use std::ffi::OsString;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;

use clap::Parser;
use nobs_cli::{run, Cli};
use nobs_core::runner::FakeRunner;

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

struct Bare(PathBuf);

impl Bare {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nobs-cli-bare-{name}-{}", std::process::id()));
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

fn nobs(args: &[&str]) -> (i32, FakeRunner) {
    let runner = FakeRunner::returning(0);
    let mut line = vec!["nobs"];
    line.extend(args);
    let code = run(Cli::try_parse_from(line).unwrap(), &runner);
    (code, runner)
}

fn binary(args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(args)
        .env_remove("NOBS_DIR")
        .env("PATH", "")
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn compose_ps(project: Option<&str>, file: &Path, services: &[&str]) -> Vec<OsString> {
    let mut list = vec![OsString::from("compose")];
    if let Some(name) = project {
        list.extend(["--project-name".into(), name.into()]);
    }
    list.extend(["-f".into(), file.as_os_str().to_owned()]);
    list.extend(["ps", "--all"].iter().chain(services).map(OsString::from));
    list
}

fn inspect(file: &Path) -> Vec<OsString> {
    vec!["inspect".into(), "-t".into(), file.as_os_str().to_owned()]
}

#[test]
fn a_workshop_name_from_below_its_folder_runs_ps_with_its_project() {
    let here = enter(&fixture("workshop-one/sub"));
    let file = here.parent().unwrap().join("docker-compose.yml");

    let (code, runner) = nobs(&["nobs-t-one", "ps"]);

    assert_eq!(code, 0);
    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "docker");
    assert_eq!(calls[0].args, compose_ps(Some("nobs-t-one"), &file, &[]));

    let (code, runner) = nobs(&["nobs-t-one", "ps", "sleeper"]);

    assert_eq!(code, 0);
    assert_eq!(
        runner.calls()[0].args,
        compose_ps(Some("nobs-t-one"), &file, &["sleeper"])
    );
}

#[test]
fn a_workshop_below_the_current_folder_is_found_and_its_missing_compose_file_named() {
    let here = enter(&fixture("workshops-repo"));
    let file = here.join("workshops/nobs-t-two/docker-compose.yml");

    let (code, runner) = nobs(&["nobs-t-two", "ps"]);

    assert_eq!(code, 1);
    assert!(runner.calls().is_empty());
    let output = binary(&["nobs-t-two", "ps"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr(&output);
    assert!(
        stderr.contains(&format!("compose file not found: {}", file.display())),
        "{stderr}"
    );
}

#[test]
fn an_unknown_workshop_name_exits_2_and_names_the_known_ones() {
    enter(&fixture("workshops-repo"));

    let (code, runner) = nobs(&["nobs-t-none", "ps"]);

    assert_eq!(code, 2);
    assert!(runner.calls().is_empty());
    let output = binary(&["nobs-t-none", "ps"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr(&output);
    assert!(stderr.contains("no workshop named nobs-t-none"), "{stderr}");
    assert!(stderr.contains("known: nobs-t-two"), "{stderr}");
}

#[test]
fn a_bad_nobs_toml_met_by_the_walk_stops_a_workshop_name_with_exit_1() {
    let bare = Bare::new("bad-toml");
    fs::write(
        bare.0.join("nobs.toml"),
        "kind = \"lab\"\ncolour = \"red\"\n",
    )
    .unwrap();
    enter(&bare.0);

    let (code, runner) = nobs(&["nobs-t-one", "ps"]);

    assert_eq!(code, 1);
    assert!(runner.calls().is_empty());
    let output = binary(&["nobs-t-one", "ps"]);
    let stderr = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("nobs.toml: colour: unknown key"),
        "{stderr}"
    );
}

#[test]
fn a_scenario_in_a_book_lab_uses_the_chapter_compose_file() {
    let here = enter(&fixture("book-lab/chapters/ch1"));

    let (code, runner) = nobs(&["docker", "ps", "-s", "ch1"]);

    assert_eq!(code, 0);
    assert_eq!(
        runner.calls()[0].args,
        compose_ps(None, &here.join("docker-compose.yml"), &[])
    );
}

#[test]
fn containerlab_in_a_book_lab_uses_lab_yml() {
    let here = enter(&fixture("book-lab/chapters/ch1"));
    let book = here.parent().unwrap().parent().unwrap();

    let (code, runner) = nobs(&["containerlab", "inspect", "--no-sudo"]);

    assert_eq!(code, 0);
    let calls = runner.calls();
    assert_eq!(calls[0].program, "containerlab");
    assert_eq!(calls[0].args, inspect(&book.join("containerlab/lab.yml")));
}

#[test]
fn containerlab_in_a_labcli_lab_falls_back_to_lab_clab_yml() {
    let here = enter(&fixture("labcli-example"));

    let (code, runner) = nobs(&["containerlab", "inspect", "--no-sudo"]);

    assert_eq!(code, 0);
    assert_eq!(
        runner.calls()[0].args,
        inspect(&here.join("containerlab/lab.clab.yml"))
    );
}

#[test]
fn containerlab_in_a_labcli_lab_prefers_lab_clab_yaml() {
    let here = enter(&fixture("labcli-yaml"));

    let (code, runner) = nobs(&["containerlab", "inspect", "--no-sudo"]);

    assert_eq!(code, 0);
    assert_eq!(
        runner.calls()[0].args,
        inspect(&here.join("containerlab/lab.clab.yaml"))
    );
}

#[test]
fn docker_in_a_lab_with_a_project_override_passes_the_project_name() {
    let here = enter(&fixture("labcli-yaml"));

    let (code, runner) = nobs(&["docker", "ps"]);

    assert_eq!(code, 0);
    assert_eq!(
        runner.calls()[0].args,
        compose_ps(Some("nobs-t-yaml"), &here.join("docker-compose.yml"), &[])
    );
}

#[test]
fn docker_in_a_workshop_folder_passes_the_workshop_project_name() {
    let here = enter(&fixture("workshop-one"));

    let (code, runner) = nobs(&["docker", "ps"]);

    assert_eq!(code, 0);
    assert_eq!(
        runner.calls()[0].args,
        compose_ps(Some("nobs-t-one"), &here.join("docker-compose.yml"), &[])
    );
}

#[test]
fn docker_and_containerlab_from_a_bare_folder_exit_1_without_a_call() {
    let bare = Bare::new("nothing");
    enter(&bare.0);

    for args in [
        &["docker", "ps"][..],
        &["containerlab", "inspect", "--no-sudo"][..],
    ] {
        let (code, runner) = nobs(args);

        assert_eq!(code, 1, "{args:?}");
        assert!(runner.calls().is_empty(), "{args:?}");
        let output = binary(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let stderr = stderr(&output);
        assert!(
            stderr.contains("nobs: no lab or workshop found from here"),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn an_explicit_compose_file_needs_no_lab_folder() {
    let bare = Bare::new("explicit");
    enter(&bare.0);
    let file = fs::canonicalize(fixture("one-service/docker-compose.yml")).unwrap();

    let (code, runner) = nobs(&["docker", "ps", "-c", file.to_str().unwrap()]);

    assert_eq!(code, 0);
    assert_eq!(runner.calls()[0].args, compose_ps(None, &file, &[]));
}

#[test]
fn docker_network_needs_no_lab_folder() {
    let bare = Bare::new("network");
    enter(&bare.0);

    let (code, runner) = nobs(&["docker", "network", "ls"]);

    assert_eq!(code, 0);
    assert_eq!(runner.calls()[0].args, ["network", "ls"]);
}

#[test]
fn sonda_needs_no_lab_folder() {
    let bare = Bare::new("sonda");
    enter(&bare.0);
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    let output = binary(&[
        "sonda",
        "--server",
        &format!("http://127.0.0.1:{port}"),
        "health",
    ]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr(&output);
    assert!(stderr.contains("cannot reach"), "{stderr}");
    assert!(!stderr.contains("no lab or workshop"), "{stderr}");
}

#[test]
fn workshops_from_a_bare_folder_prints_nothing() {
    let bare = Bare::new("workshops");
    enter(&bare.0);

    let output = binary(&["workshops"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
}

#[test]
fn dir_names_the_workshop_folder_from_a_bare_folder() {
    let bare = Bare::new("dir-workshop");
    enter(&bare.0);
    let workshop = fs::canonicalize(fixture("workshop-one")).unwrap();

    let (code, runner) = nobs(&["--dir", workshop.to_str().unwrap(), "nobs-t-one", "ps"]);

    assert_eq!(code, 0);
    assert_eq!(
        runner.calls()[0].args,
        compose_ps(
            Some("nobs-t-one"),
            &workshop.join("docker-compose.yml"),
            &[]
        )
    );
}

#[test]
fn dir_naming_a_bare_folder_exits_1_and_names_it() {
    let bare = Bare::new("dir-bare");
    let here = enter(&bare.0);
    let named = here.to_str().unwrap();

    let (code, runner) = nobs(&["--dir", named, "docker", "ps"]);

    assert_eq!(code, 1);
    assert!(runner.calls().is_empty());
    let output = binary(&["--dir", named, "docker", "ps"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr(&output);
    assert!(
        stderr.contains(&format!("nobs: no lab or workshop at {named}")),
        "{stderr}"
    );
}

#[test]
fn a_missing_scenario_compose_file_in_a_book_lab_exits_1_and_names_it() {
    let here = enter(&fixture("book-lab"));
    let file = here.join("chapters/ch9/docker-compose.yml");

    let (code, runner) = nobs(&["docker", "ps", "-s", "ch9"]);

    assert_eq!(code, 1);
    assert!(runner.calls().is_empty());
    let output = binary(&["docker", "ps", "-s", "ch9"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr(&output);
    assert!(
        stderr.contains(&format!("nobs: compose file not found: {}", file.display())),
        "{stderr}"
    );
}

#[test]
fn workshops_with_dir_lists_the_named_workshop_from_a_bare_folder() {
    let bare = Bare::new("workshops-dir");
    enter(&bare.0);
    let workshop = fs::canonicalize(fixture("workshop-one")).unwrap();

    let output = binary(&["--dir", workshop.to_str().unwrap(), "workshops"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("nobs-t-one  Workshop one  {}\n", workshop.display())
    );
}

#[test]
fn workshops_from_inside_a_workshop_lists_it_once() {
    let here = enter(&fixture("workshop-one/sub"));

    let output = binary(&["workshops"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!(
            "nobs-t-one  Workshop one  {}\n",
            here.parent().unwrap().display()
        )
    );
}

#[test]
fn workshops_with_dir_naming_a_bare_folder_exits_1_and_names_it() {
    let bare = Bare::new("workshops-dir-bare");
    let here = enter(&bare.0);
    let named = here.to_str().unwrap();

    let output = binary(&["--dir", named, "workshops"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr(&output);
    assert!(
        stderr.contains(&format!("nobs: no lab or workshop at {named}")),
        "{stderr}"
    );
}

#[test]
fn workshops_stops_with_exit_1_on_a_bad_nobs_toml_found_upward() {
    let bare = Bare::new("workshops-bad-toml");
    fs::write(
        bare.0.join("nobs.toml"),
        "kind = \"lab\"\ncolour = \"red\"\n",
    )
    .unwrap();
    fs::create_dir_all(bare.0.join("sub")).unwrap();
    let here = enter(&bare.0.join("sub"));
    let file = here.parent().unwrap().join("nobs.toml");

    let output = binary(&["workshops"]);

    let stderr = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!("nobs: {}: colour: unknown key", file.display())),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn a_workshop_name_with_dir_naming_a_bare_folder_exits_1_and_names_it() {
    let bare = Bare::new("dispatch-dir-bare");
    let here = enter(&bare.0);
    let named = here.to_str().unwrap();

    let (code, runner) = nobs(&["--dir", named, "nobs-t-one", "ps"]);

    assert_eq!(code, 1);
    assert!(runner.calls().is_empty());
    let output = binary(&["--dir", named, "nobs-t-one", "ps"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr(&output);
    assert!(
        stderr.contains(&format!("nobs: no lab or workshop at {named}")),
        "{stderr}"
    );
}

#[test]
fn a_bad_nobs_toml_found_upward_stops_workshop_commands_even_with_dir() {
    let bare = Bare::new("bad-toml-with-dir");
    fs::write(
        bare.0.join("nobs.toml"),
        "kind = \"lab\"\ncolour = \"red\"\n",
    )
    .unwrap();
    fs::create_dir_all(bare.0.join("sub")).unwrap();
    let here = enter(&bare.0.join("sub"));
    let file = here.parent().unwrap().join("nobs.toml");
    let good = fs::canonicalize(fixture("workshop-one")).unwrap();
    let good = good.to_str().unwrap();

    for args in [
        &["--dir", good, "workshops"][..],
        &["--dir", good, "nobs-t-one", "ps"][..],
    ] {
        let (code, runner) = nobs(args);

        assert_eq!(code, 1, "{args:?}");
        assert!(runner.calls().is_empty(), "{args:?}");
        let output = binary(args);
        let stderr = stderr(&output);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.contains(&format!("nobs: {}: colour: unknown key", file.display())),
            "{args:?}: {stderr}"
        );
    }
}
