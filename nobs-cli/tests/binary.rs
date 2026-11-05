use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/one-service/docker-compose.yml")
}

#[test]
fn a_missing_compose_file_exits_1_and_names_the_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--compose", "missing.yml"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("compose file not found"), "{stderr}");
    assert!(stderr.contains("missing.yml"), "{stderr}");
}

#[test]
fn nobs_exits_1_and_says_so_when_docker_cannot_start() {
    let empty = empty_folder();
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--compose"])
        .arg(fixture())
        .env("PATH", &empty)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot run docker"), "{stderr}");
}

#[test]
fn docker_ps_from_a_bare_folder_exits_1_and_says_no_lab_was_found() {
    let output = nobs_from_a_bare_folder(&["docker", "ps"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no lab or workshop found from here"),
        "{stderr}"
    );
    assert!(!stderr.contains("cannot run"), "{stderr}");
}

#[test]
fn a_scenario_from_a_bare_folder_exits_1_and_says_no_lab_was_found() {
    let output = nobs_from_a_bare_folder(&["docker", "ps", "--scenario", "ch5"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no lab or workshop found from here"),
        "{stderr}"
    );
    assert!(!stderr.contains("cannot run"), "{stderr}");
}

#[test]
fn every_command_exits_with_the_code_of_the_docker_program_on_path() {
    let dir = fake_docker("passthrough");
    let path = path_with(&dir);
    let fixture = fixture();
    let file = fixture.to_str().unwrap();
    let commands: [Vec<&str>; 13] = [
        vec!["build", "--compose", file],
        vec!["pull", "--compose", file],
        vec!["exec", "--compose", file, "sleeper", "true"],
        vec!["debug", "--compose", file],
        vec!["start", "--compose", file],
        vec!["stop", "--compose", file],
        vec!["restart", "--compose", file],
        vec!["logs", "--compose", file],
        vec!["ps", "--compose", file],
        vec!["destroy", "--compose", file],
        vec!["rm", "--compose", file],
        vec!["update", "--compose", file],
        vec!["network", "ls"],
    ];

    for command in commands {
        let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
            .arg("docker")
            .args(&command)
            .env("PATH", &path)
            .env("NOBS_T_CODE", "5")
            .output()
            .unwrap();

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(5), "{command:?}: {stderr}");
    }
}

#[test]
fn a_value_from_dot_env_reaches_the_docker_child() {
    let dir = fake_docker("dot-env");
    std::fs::write(dir.join(".env"), "NOBS_T_CODE=5\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--compose"])
        .arg(fixture())
        .current_dir(&dir)
        .env("PATH", path_with(&dir))
        .env_remove("NOBS_T_CODE")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(5), "{stderr}");
}

#[test]
fn an_empty_key_in_dot_env_exits_1_before_docker_starts() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cli-empty-key");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".env"), "=x\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps", "--compose"])
        .arg(fixture())
        .current_dir(&dir)
        .env("PATH", empty_folder())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("empty key"), "{stderr}");
    assert!(!stderr.contains("cannot run docker"), "{stderr}");
}

#[test]
fn a_missing_topology_exits_1_and_names_the_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["containerlab", "deploy", "missing.clab.yml"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("topology file not found"), "{stderr}");
    assert!(stderr.contains("missing.clab.yml"), "{stderr}");
}

#[test]
fn nobs_exits_1_and_says_so_when_sudo_cannot_start() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["containerlab", "deploy"])
        .arg(topology())
        .env("PATH", empty_folder())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot run sudo"), "{stderr}");
}

#[test]
fn containerlab_deploy_from_a_bare_folder_exits_1_and_says_no_lab_was_found() {
    let output = nobs_from_a_bare_folder(&["containerlab", "deploy"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no lab or workshop found from here"),
        "{stderr}"
    );
    assert!(!stderr.contains("cannot run"), "{stderr}");
}

#[test]
fn every_containerlab_command_gives_sudo_the_variable_names_and_exits_with_its_code() {
    let dir = fake_sudo("fake-sudo");
    let seen = dir.join("seen");
    let topology = topology();
    let cases = [("deploy", ""), ("destroy", " --cleanup"), ("inspect", "")];

    for (command, tail) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
            .args(["containerlab", command])
            .arg(&topology)
            .env("PATH", path_with(&dir))
            .env("NOBS_T_LABEL", "nobs-t-test")
            .output()
            .unwrap();

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(5), "{command}: {stderr}");
        assert_eq!(
            std::fs::read_to_string(&seen).unwrap(),
            format!(
                "--preserve-env=NOBS_T_LABEL containerlab {command} -t {}{tail}\nnobs-t-test\n",
                topology.display()
            ),
            "{command}"
        );
    }
}

#[test]
fn a_value_from_dot_env_reaches_the_containerlab_child() {
    let dir = fake_sudo("fake-sudo-dot-env");
    std::fs::write(dir.join(".env"), "NOBS_T_LABEL=nobs-t-from-dot-env\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["containerlab", "deploy"])
        .arg(topology())
        .current_dir(&dir)
        .env("PATH", path_with(&dir))
        .env_remove("NOBS_T_LABEL")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(5), "{stderr}");
    let seen = std::fs::read_to_string(dir.join("seen")).unwrap();
    assert_eq!(seen.lines().nth(1), Some("nobs-t-from-dot-env"), "{seen}");
}

#[test]
fn the_address_in_dot_env_is_used_when_nothing_else_is_given() {
    let (a, b) = (health_stub(), health_stub());
    let dir = sonda_folder(
        "sonda-dot-env-address",
        Some(&format!("SONDA_SERVER_URL={}\n", a.url())),
    );

    let output = nobs_sonda(&dir, &["health"], &[]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!((a.requests().len(), b.requests().len()), (1, 0));
}

#[test]
fn the_address_in_the_shell_beats_dot_env() {
    let (a, b) = (health_stub(), health_stub());
    let dir = sonda_folder(
        "sonda-shell-address",
        Some(&format!("SONDA_SERVER_URL={}\n", a.url())),
    );

    let output = nobs_sonda(&dir, &["health"], &[("SONDA_SERVER_URL", b.url())]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!((a.requests().len(), b.requests().len()), (0, 1));
}

#[test]
fn the_server_flag_beats_the_shell_address() {
    let (a, b) = (health_stub(), health_stub());
    let dir = sonda_folder("sonda-flag-address", None);

    let output = nobs_sonda(
        &dir,
        &["--server", a.url(), "health"],
        &[("SONDA_SERVER_URL", b.url())],
    );

    assert_eq!(output.status.code(), Some(0));
    assert_eq!((a.requests().len(), b.requests().len()), (1, 0));
}

#[test]
fn an_empty_server_flag_counts_as_not_given() {
    let (a, b) = (health_stub(), health_stub());
    let dir = sonda_folder("sonda-empty-address", None);

    let output = nobs_sonda(
        &dir,
        &["--server", "", "health"],
        &[("SONDA_SERVER_URL", b.url())],
    );

    assert_eq!(output.status.code(), Some(0));
    assert_eq!((a.requests().len(), b.requests().len()), (0, 1));
}

#[test]
fn the_key_in_dot_env_is_sent_as_a_bearer_token() {
    let a = list_stub();
    let dir = sonda_folder("sonda-dot-env-key", Some("SONDA_API_KEY=k\n"));

    let output = nobs_sonda(&dir, &["--server", a.url(), "list"], &[]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(a.requests()[0].header("Authorization"), Some("Bearer k"));
}

#[test]
fn the_api_key_flag_beats_the_shell_key() {
    let a = list_stub();
    let dir = sonda_folder("sonda-flag-key", None);

    let output = nobs_sonda(
        &dir,
        &["--server", a.url(), "--api-key", "f", "list"],
        &[("SONDA_API_KEY", "s")],
    );

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(a.requests()[0].header("Authorization"), Some("Bearer f"));
}

#[test]
fn an_empty_key_in_dot_env_sends_no_authorization_header() {
    let a = list_stub();
    let dir = sonda_folder("sonda-empty-key", Some("SONDA_API_KEY=\n"));

    let output = nobs_sonda(&dir, &["--server", a.url(), "list"], &[]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(a.requests()[0].header("Authorization"), None);
}

#[test]
fn an_empty_shell_address_lets_dot_env_give_the_address() {
    let a = health_stub();
    let dir = sonda_folder(
        "sonda-empty-shell-address",
        Some(&format!("SONDA_SERVER_URL={}\n", a.url())),
    );

    let output = nobs_sonda(&dir, &["health"], &[("SONDA_SERVER_URL", "")]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(a.requests().len(), 1);
}

#[test]
fn an_empty_shell_key_lets_dot_env_give_the_key() {
    let a = list_stub();
    let dir = sonda_folder("sonda-empty-shell-key", Some("SONDA_API_KEY=k\n"));

    let output = nobs_sonda(
        &dir,
        &["--server", a.url(), "list"],
        &[("SONDA_API_KEY", "")],
    );

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(a.requests()[0].header("Authorization"), Some("Bearer k"));
}

#[test]
fn the_dot_env_of_the_workshop_folder_reaches_docker_from_below_it() {
    let copy = workshop_copy("workshop-below");
    let bin = probe_docker("probe-docker-below");

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["nobs-t-one", "ps"])
        .current_dir(copy.join("sub"))
        .env("PATH", path_with(&bin))
        .env_remove("NOBS_T_PROBE")
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let folder = std::fs::canonicalize(&copy).unwrap();
    assert_eq!(
        std::fs::read_to_string(bin.join("seen")).unwrap(),
        format!(
            "compose --project-name nobs-t-one -f {}/docker-compose.yml ps --all\nfrom-env\n",
            folder.display()
        )
    );
}

#[test]
fn nobs_dir_names_the_workshop_folder_from_a_bare_folder() {
    let copy = workshop_copy("workshop-nobs-dir");
    let bin = probe_docker("probe-docker-nobs-dir");

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["nobs-t-one", "ps"])
        .current_dir(empty_folder())
        .env("PATH", path_with(&bin))
        .env("NOBS_DIR", &copy)
        .env_remove("NOBS_T_PROBE")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(bin.join("seen")).unwrap(),
        format!(
            "compose --project-name nobs-t-one -f {}/docker-compose.yml ps --all\nfrom-env\n",
            copy.display()
        )
    );
}

#[test]
fn the_dot_env_of_the_lab_folder_reaches_docker_compose_from_below_it() {
    let copy = workshop_copy("workshop-docker-below");
    let bin = probe_docker("probe-docker-compose-below");

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["docker", "ps"])
        .current_dir(copy.join("sub"))
        .env("PATH", path_with(&bin))
        .env_remove("NOBS_T_PROBE")
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let folder = std::fs::canonicalize(&copy).unwrap();
    assert_eq!(
        std::fs::read_to_string(bin.join("seen")).unwrap(),
        format!(
            "compose --project-name nobs-t-one -f {}/docker-compose.yml ps --all\nfrom-env\n",
            folder.display()
        )
    );
}

#[test]
fn the_dot_env_of_a_workshop_found_below_the_current_folder_reaches_docker() {
    let copy = workshop_copy("workshop-scan/nobs-t-one");
    let bin = probe_docker("probe-docker-scan");

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["nobs-t-one", "ps"])
        .current_dir(copy.parent().unwrap())
        .env("PATH", path_with(&bin))
        .env_remove("NOBS_T_PROBE")
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let folder = std::fs::canonicalize(&copy).unwrap();
    assert_eq!(
        std::fs::read_to_string(bin.join("seen")).unwrap(),
        format!(
            "compose --project-name nobs-t-one -f {}/docker-compose.yml ps --all\nfrom-env\n",
            folder.display()
        )
    );
}

#[test]
fn workshops_lists_the_workshop_below_the_current_folder() {
    let repo = fixture_folder("workshops-repo");

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .arg("workshops")
        .current_dir(&repo)
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let folder = std::fs::canonicalize(repo.join("workshops/nobs-t-two")).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("nobs-t-two  Workshop two  {}\n", folder.display())
    );
}

#[test]
fn a_workshop_help_page_lists_ps() {
    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["nobs-t-one", "--help"])
        .current_dir(fixture_folder("workshop-one"))
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout
            .lines()
            .any(|line| line.trim_start().starts_with("ps ")),
        "{stdout}"
    );
}

#[test]
fn workshop_setup_copies_the_example_once_and_then_keeps_the_env_file() {
    let copy = example_copy("setup-copy");
    let folder = std::fs::canonicalize(&copy).unwrap();

    let first = nobs_in(&copy, &["nobs-t-one", "setup"]);

    assert_eq!(first.status.code(), Some(0), "{}", stderr_of(&first));
    assert_eq!(
        String::from_utf8_lossy(&first.stdout),
        format!("created {}/.env from .env.example\n", folder.display())
    );
    assert_eq!(
        std::fs::read(copy.join(".env")).unwrap(),
        std::fs::read(fixture_folder("workshop-one").join(".env.example")).unwrap()
    );

    let second = nobs_in(&copy, &["nobs-t-one", "setup"]);

    assert_eq!(second.status.code(), Some(0), "{}", stderr_of(&second));
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        format!("kept {}/.env\n", folder.display())
    );
    std::fs::remove_dir_all(&copy).unwrap();
}

#[test]
fn a_bare_up_from_below_a_workshop_runs_compose_up_and_prints_the_seven_addresses() {
    let copy = workshop_copy("workshop-up");
    let bin = probe_docker("probe-docker-up");

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .arg("up")
        .current_dir(copy.join("sub"))
        .env("PATH", path_with(&bin))
        .env_remove("NOBS_T_PROBE")
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    let folder = std::fs::canonicalize(&copy).unwrap();
    assert_eq!(
        std::fs::read_to_string(bin.join("seen")).unwrap(),
        format!(
            "compose --project-name nobs-t-one -f {}/docker-compose.yml up -d\nfrom-env\n",
            folder.display()
        )
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Grafana       http://localhost:3000\n\
         Prometheus    http://localhost:9090\n\
         Alertmanager  http://localhost:9093\n\
         Loki          http://localhost:3001\n\
         Infrahub      http://localhost:8000\n\
         Prefect       http://localhost:4200\n\
         Sonda         http://localhost:8085\n"
    );
}

#[test]
fn a_failing_docker_makes_up_exit_with_its_code_and_print_no_address() {
    let copy = workshop_copy("workshop-up-fails");
    let bin = fake_docker("fake-docker-up-fails");

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["nobs-t-one", "up"])
        .current_dir(&copy)
        .env("PATH", path_with(&bin))
        .env("NOBS_T_CODE", "3")
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{}", stderr_of(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("http://"), "{stdout}");
}

#[test]
fn a_foreign_stack_stops_up_with_a_message_naming_it_and_its_containers() {
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("workshop-guard");
    let _ = std::fs::remove_dir_all(&scratch);
    let two = scratch.join("workshops/nobs-t-two");
    let one = scratch.join("workshops/nobs-t-one");
    std::fs::create_dir_all(&two).unwrap();
    std::fs::create_dir_all(&one).unwrap();
    std::fs::copy(
        fixture_folder("workshops-repo/workshops/nobs-t-two/nobs.toml"),
        two.join("nobs.toml"),
    )
    .unwrap();
    for file in ["nobs.toml", "docker-compose.yml"] {
        std::fs::copy(fixture_folder("workshop-one").join(file), one.join(file)).unwrap();
    }
    let bin = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("guard-docker");
    std::fs::create_dir_all(&bin).unwrap();
    let _ = std::fs::remove_file(bin.join("seen"));
    let script = bin.join("docker");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ \"$1\" = ps ]; then echo sonda-server; exit 0; fi\necho \"$*\" >> '{seen}'\n",
            seen = bin.join("seen").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["nobs-t-one", "up"])
        .current_dir(&scratch)
        .env("PATH", path_with(&bin))
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr_of(&output),
        "nobs: containers of the Workshop two stack are present and share names and ports with this one: sonda-server; run nobs nobs-t-two down first\n"
    );
    assert!(output.stdout.is_empty());
    assert!(!bin.join("seen").exists());
    std::fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn a_guard_question_that_fails_still_lets_up_go_on_and_shows_the_docker_error() {
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("workshop-guard-fails");
    let _ = std::fs::remove_dir_all(&scratch);
    let two = scratch.join("workshops/nobs-t-two");
    let one = scratch.join("workshops/nobs-t-one");
    std::fs::create_dir_all(&two).unwrap();
    std::fs::create_dir_all(&one).unwrap();
    std::fs::copy(
        fixture_folder("workshops-repo/workshops/nobs-t-two/nobs.toml"),
        two.join("nobs.toml"),
    )
    .unwrap();
    for file in ["nobs.toml", "docker-compose.yml"] {
        std::fs::copy(fixture_folder("workshop-one").join(file), one.join(file)).unwrap();
    }
    let bin = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("guard-docker-fails");
    std::fs::create_dir_all(&bin).unwrap();
    let _ = std::fs::remove_file(bin.join("seen"));
    let script = bin.join("docker");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ \"$1\" = ps ]; then echo sonda-server; echo nobs-t-guard-stderr >&2; exit 3; fi\necho \"$*\" >> '{seen}'\n",
            seen = bin.join("seen").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(["nobs-t-one", "up"])
        .current_dir(&scratch)
        .env("PATH", path_with(&bin))
        .env_remove("NOBS_DIR")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    assert_eq!(
        std::fs::read_to_string(bin.join("seen")).unwrap(),
        format!(
            "compose --project-name nobs-t-one -f {}/docker-compose.yml up -d\n",
            std::fs::canonicalize(&one).unwrap().display()
        )
    );
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("nobs-t-guard-stderr"),
        "stderr of nobs was {stderr:?}"
    );
    std::fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn a_bare_up_from_a_bare_folder_says_no_workshop_named_up() {
    let output = nobs_from_a_bare_folder(&["up"]);

    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(stderr.contains("no workshop named up"), "{stderr}");
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn nobs_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(args)
        .current_dir(dir)
        .env_remove("NOBS_DIR")
        .output()
        .unwrap()
}

fn example_copy(name: &str) -> PathBuf {
    let copy = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&copy);
    std::fs::create_dir_all(copy.join("sub")).unwrap();
    for file in ["nobs.toml", "docker-compose.yml", ".env.example"] {
        std::fs::copy(fixture_folder("workshop-one").join(file), copy.join(file)).unwrap();
    }
    copy
}

fn nobs_from_a_bare_folder(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(args)
        .current_dir(empty_folder())
        .env("PATH", empty_folder())
        .env_remove("NOBS_DIR")
        .output()
        .unwrap()
}

fn fixture_folder(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(name)
}

fn workshop_copy(name: &str) -> PathBuf {
    let copy = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&copy);
    std::fs::create_dir_all(copy.join("sub")).unwrap();
    for file in ["nobs.toml", "docker-compose.yml"] {
        std::fs::copy(fixture_folder("workshop-one").join(file), copy.join(file)).unwrap();
    }
    std::fs::write(copy.join(".env"), "NOBS_T_PROBE=from-env\n").unwrap();
    copy
}

fn probe_docker(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let _ = std::fs::remove_file(dir.join("seen"));
    let script = dir.join("docker");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$*\" > '{seen}'\necho \"$NOBS_T_PROBE\" >> '{seen}'\n",
            seen = dir.join("seen").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn health_stub() -> Stub {
    Stub::start(&[("GET", "/health", 200, r#"{"status":"ok"}"#)])
}

fn list_stub() -> Stub {
    Stub::start(&[("GET", "/scenarios", 200, r#"{"scenarios":[]}"#)])
}

fn sonda_folder(name: &str, dot_env: Option<&str>) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    match dot_env {
        Some(text) => std::fs::write(dir.join(".env"), text).unwrap(),
        None => {
            let _ = std::fs::remove_file(dir.join(".env"));
        }
    }
    dir
}

fn nobs_sonda(dir: &Path, args: &[&str], shell: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nobs"))
        .arg("sonda")
        .args(args)
        .current_dir(dir)
        .env_remove("SONDA_SERVER_URL")
        .env_remove("SONDA_API_KEY")
        .envs(shell.iter().copied())
        .output()
        .unwrap()
}

fn fake_sudo(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("sudo");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$*\" > '{seen}'\necho \"$NOBS_T_LABEL\" >> '{seen}'\nexit 5\n",
            seen = dir.join("seen").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn topology() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/srlinux-two-node/lab.clab.yml")
}

fn empty_folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("empty-path");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fake_docker(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("docker");
    std::fs::write(&script, "#!/bin/sh\nexit \"${NOBS_T_CODE:-9}\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn path_with(dir: &Path) -> OsString {
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}
