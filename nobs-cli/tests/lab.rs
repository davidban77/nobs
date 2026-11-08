use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const FAKE: &str = r#"#!/bin/sh
here=${0%/*}
end=$(printf '\037')
seen=0
if [ -f "$here/calls" ]; then
  while IFS= read -r line; do
    if [ "$line" = "$end" ]; then seen=$((seen + 1)); fi
  done < "$here/calls"
fi
{
  printf '\036%s\n' "${0##*/}"
  for argument in "$@"; do printf '%s\n' "$argument"; done
  printf '%s\n' "$end"
} >> "$here/calls"
index=0
while IFS= read -r step || [ -n "$step" ]; do
  if [ "$index" = "$seen" ]; then
    printf '%b' "${step#*|}"
    exit "${step%%|*}"
  fi
  index=$((index + 1))
done < "$here/script"
exit 0
"#;

const SERVICE_UP: &str = r#"{"Service":"sleeper","State":"running","Health":""}"#;
const SERVICE_EXITED: &str = r#"{"Service":"sleeper","State":"exited","Health":""}"#;
const NODES_UP: &str = r#"{"nobs-t-book":[{"name":"clab-nobs-t-book-nobs-t-srl1","state":"running"},{"name":"clab-nobs-t-book-nobs-t-srl2","state":"running"}]}"#;
const NODE_ONE_UP: &str =
    r#"{"nobs-t-book":[{"name":"clab-nobs-t-book-nobs-t-srl1","state":"running"}]}"#;
const OTHER_TOPOLOGY: &str =
    "name: other\ntopology:\n  nodes:\n    n1:\n      kind: nokia_srlinux\n";

const START: char = '\u{1e}';
const END: &str = "\u{1f}";

const CONTAINERS: &[&str] = &["docker", "ps", "-a", "--format", "{{.Names}}"];
const PROJECT_CONTAINERS: &[&str] = &[
    "docker",
    "ps",
    "-a",
    "--filter",
    "label=com.docker.compose.project=netobs",
    "--format",
    "{{.Names}}",
];
const NETWORKS: &[&str] = &["docker", "network", "ls", "--format", "{{.Name}}"];
const CREATE: &[&str] = &[
    "docker",
    "network",
    "create",
    "--driver=bridge",
    "--subnet=198.51.100.0/24",
    "network-observability",
];

type Step = (i32, &'static str);

type Call = Vec<String>;

fn call(words: &[&str]) -> Call {
    words.iter().map(|word| word.to_string()).collect()
}

fn docker_compose(project: &str, file: &Path, rest: &[&str]) -> Call {
    let mut words = vec![
        "docker",
        "compose",
        "--project-name",
        project,
        "-f",
        file.to_str().unwrap(),
    ];
    words.extend(rest);
    call(&words)
}

fn containerlab(topology: &Path, action: &str, rest: &[&str]) -> Call {
    let mut words = vec!["containerlab", action, "-t", topology.to_str().unwrap()];
    words.extend(rest);
    call(&words)
}

fn sudo_containerlab(topology: &Path, action: &str, rest: &[&str]) -> Call {
    let mut words = vec![
        "sudo",
        "--preserve-env=NOBS_T_LABEL",
        "containerlab",
        action,
        "-t",
        topology.to_str().unwrap(),
    ];
    words.extend(rest);
    call(&words)
}

type Flags<'a> = &'a [&'a str];

type Env<'a> = &'a [(&'a str, &'a str)];

fn ready(changes: &[(usize, i32, &'static str)]) -> Vec<Step> {
    let mut steps = vec![(0, ""); 6];
    steps.push((0, SERVICE_UP));
    steps.push((0, NODES_UP));
    for (index, code, text) in changes {
        steps[*index] = (*code, text);
    }
    steps
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

struct Lab(PathBuf);

impl Lab {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("nobs-cli-lab-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).unwrap();
        let lab = Self(root.canonicalize().unwrap());
        let book = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/book-lab");
        for file in ["chapters/ch1/docker-compose.yml", "containerlab/lab.yml"] {
            lab.file(file, &fs::read_to_string(book.join(file)).unwrap());
        }
        for program in ["docker", "containerlab", "sudo"] {
            let path = lab.bin().join(program);
            fs::write(&path, FAKE).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        lab.script(&[]);
        lab
    }

    fn dir(&self) -> PathBuf {
        self.0.join("lab")
    }

    fn bin(&self) -> PathBuf {
        self.0.join("bin")
    }

    fn topology(&self) -> PathBuf {
        self.dir().join("containerlab/lab.yml")
    }

    fn compose(&self, scenario: &str) -> PathBuf {
        self.dir()
            .join("chapters")
            .join(scenario)
            .join("docker-compose.yml")
    }

    fn file(&self, relative: &str, text: &str) {
        let path = self.dir().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn script(&self, steps: &[Step]) {
        let text: String = steps
            .iter()
            .map(|(code, stdout)| format!("{code}|{}\n", stdout.replace('\n', "\\n")))
            .collect();
        fs::write(self.bin().join("script"), text).unwrap();
        fs::write(self.bin().join("calls"), "").unwrap();
    }

    fn without(&self, program: &str) {
        fs::remove_file(self.bin().join(program)).unwrap();
    }

    fn calls(&self) -> Vec<Call> {
        let text = fs::read_to_string(self.bin().join("calls")).unwrap();
        let mut calls = Vec::new();
        let mut current = Vec::new();
        for line in text.lines() {
            if let Some(program) = line.strip_prefix(START) {
                current = vec![program.to_string()];
            } else if line == END {
                calls.push(std::mem::take(&mut current));
            } else {
                current.push(line.to_string());
            }
        }
        calls
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Run {
        self.run_from(&self.dir(), args, env)
    }

    fn run_from(&self, from: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_nobs"))
            .args(args)
            .current_dir(from)
            .env_clear()
            .env("PATH", self.bin())
            .envs(env.iter().copied())
            .output()
            .unwrap();
        Run {
            code: output.status.code().unwrap(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture(relative: &str) -> PathBuf {
    fs::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures")
            .join(relative),
    )
    .unwrap()
}

#[test]
fn deploy_checks_then_creates_deploys_starts_and_waits_and_says_so() {
    let lab = Lab::new("deploy");
    lab.script(&ready(&[]));
    let (topology, ch1) = (lab.topology(), lab.compose("ch1"));

    let run = lab.run(&["lab", "deploy", "-s", "ch1"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            call(CONTAINERS),
            call(PROJECT_CONTAINERS),
            call(NETWORKS),
            call(CREATE),
            containerlab(&topology, "deploy", &[]),
            docker_compose("netobs", &ch1, &["up", "-d", "--remove-orphans"]),
            docker_compose("netobs", &ch1, &["ps", "--all", "--format", "json"]),
            containerlab(&topology, "inspect", &["--format", "json"]),
        ]
    );
    assert_eq!(
        run.stdout,
        format!(
            "lab deploy: creating network network-observability (198.51.100.0/24)\n\
             lab deploy: deploying topology {}\n\
             lab deploy: starting ch1 (project netobs)\n\
             lab deploy: waiting for 1 services and 2 nodes (up to 300s)\n\
             lab deploy: ready after 0s\n\
             lab deployed: scenario ch1\n",
            topology.display()
        )
    );
    assert_eq!(run.stderr, "");
}

#[test]
fn sudo_comes_from_the_last_flag_then_lab_sudo_and_is_off_by_default() {
    let lab = Lab::new("sudo");
    let topology = lab.topology();
    let sudo = sudo_containerlab(&topology, "deploy", &[]);
    let plain = containerlab(&topology, "deploy", &[]);
    let cases: [(Flags, Env, &Call); 6] = [
        (&["--sudo"], &[], &sudo),
        (&[], &[("LAB_SUDO", "yes")], &sudo),
        (&["--no-sudo"], &[("LAB_SUDO", "true")], &plain),
        (&[], &[], &plain),
        (&["--no-sudo", "--sudo"], &[], &sudo),
        (&["--sudo", "--no-sudo"], &[], &plain),
    ];

    for (flags, env, expected) in cases {
        lab.script(&[]);
        let mut args = vec!["lab", "deploy", "-s", "ch1", "--wait", "0"];
        args.extend(flags);

        let run = lab.run(&args, env);

        assert_eq!(run.code, 0, "{flags:?} {env:?}: {}", run.stderr);
        assert_eq!(&lab.calls()[4], expected, "{flags:?} {env:?}");
    }

    lab.script(&ready(&[]));
    lab.run(&["lab", "deploy", "-s", "ch1", "--sudo"], &[]);
    assert_eq!(
        lab.calls()[7],
        sudo_containerlab(&topology, "inspect", &["--format", "json"])
    );
}

#[test]
fn sudo_reaches_the_containerlab_calls_of_destroy_show_and_purge() {
    let lab = Lab::new("sudo-others");
    let (topology, ch1) = (lab.topology(), lab.compose("ch1"));
    let down = docker_compose("netobs", &ch1, &["down", "--volumes", "--remove-orphans"]);
    let cases: [(Flags, Vec<Step>, Vec<Call>); 3] = [
        (
            &["destroy", "-s", "ch1"],
            vec![],
            vec![
                down.clone(),
                sudo_containerlab(&topology, "destroy", &["--cleanup"]),
            ],
        ),
        (
            &["show", "-s", "ch1"],
            vec![],
            vec![
                docker_compose("netobs", &ch1, &["ps", "--all"]),
                sudo_containerlab(&topology, "inspect", &[]),
            ],
        ),
        (
            &["purge"],
            vec![(0, ""), (0, "clab-nobs-t-book-nobs-t-srl1\n")],
            vec![
                down,
                call(CONTAINERS),
                sudo_containerlab(&topology, "destroy", &["--cleanup"]),
            ],
        ),
    ];

    for (command, steps, expected) in cases {
        lab.script(&steps);
        let mut args = vec!["lab"];
        args.extend(command);
        args.push("--sudo");

        let run = lab.run(&args, &[]);

        assert_eq!(run.code, 0, "{command:?}: {}", run.stderr);
        assert_eq!(lab.calls(), expected, "{command:?}");
    }
}

#[test]
fn network_and_subnet_come_from_the_flag_then_the_variable_then_the_setting_then_the_default() {
    let lab = Lab::new("network");
    lab.file("nobs.toml", "kind = \"lab\"\nnetwork = \"settingnet\"\n");
    let created = |subnet: &'static str, name: &'static str| {
        call(&[
            "docker",
            "network",
            "create",
            "--driver=bridge",
            subnet,
            name,
        ])
    };
    let cases: [(Flags, Env, Call); 4] = [
        (
            &["--network-name", "flagnet", "--subnet", "10.1.0.0/24"],
            &[
                ("LAB_NETWORK_NAME", "envnet"),
                ("LAB_SUBNET", "10.2.0.0/24"),
            ],
            created("--subnet=10.1.0.0/24", "flagnet"),
        ),
        (
            &[],
            &[
                ("LAB_NETWORK_NAME", "envnet"),
                ("LAB_SUBNET", "10.2.0.0/24"),
            ],
            created("--subnet=10.2.0.0/24", "envnet"),
        ),
        (&[], &[], created("--subnet=198.51.100.0/24", "settingnet")),
        (
            &[],
            &[("LAB_NETWORK_NAME", ""), ("LAB_SUBNET", "  ")],
            created("--subnet=198.51.100.0/24", "settingnet"),
        ),
    ];

    for (flags, env, expected) in cases {
        lab.script(&[]);
        let mut args = vec!["lab", "deploy", "-s", "ch1", "--wait", "0"];
        args.extend(flags);

        let run = lab.run(&args, env);

        assert_eq!(run.code, 0, "{flags:?} {env:?}: {}", run.stderr);
        assert_eq!(lab.calls()[3], expected, "{flags:?} {env:?}");
    }
}

#[test]
fn the_scenario_comes_from_the_flag_then_lab_scenario_and_a_blank_one_counts_as_not_given() {
    let lab = Lab::new("scenario");
    lab.file("chapters/ch2/docker-compose.yml", "services: {}\n");
    let ch2 = lab.compose("ch2");
    let up = docker_compose("netobs", &ch2, &["up", "-d", "--remove-orphans"]);
    let cases: [(Flags, Env); 3] = [
        (&["-s", "ch2"], &[("LAB_SCENARIO", "ch1")]),
        (&[], &[("LAB_SCENARIO", "ch2")]),
        (&["-s", ""], &[("LAB_SCENARIO", "ch2")]),
    ];

    for (flags, env) in cases {
        lab.script(&[]);
        let mut args = vec!["lab", "deploy", "--wait", "0"];
        args.extend(flags);

        let run = lab.run(&args, env);

        assert_eq!(run.code, 0, "{flags:?} {env:?}: {}", run.stderr);
        assert_eq!(lab.calls()[5], up, "{flags:?} {env:?}");
    }
}

#[test]
fn the_project_and_topology_settings_apply_and_the_topology_flag_starts_at_the_lab_folder() {
    let lab = Lab::new("settings");
    lab.file(
        "nobs.toml",
        "kind = \"lab\"\nproject = \"p\"\ntopology = \"t/set.yml\"\ncompose = \"ignored.yml\"\n",
    );
    lab.file("t/set.yml", OTHER_TOPOLOGY);
    lab.file("t/flag.yml", OTHER_TOPOLOGY);
    let below = lab.dir().join("chapters/ch1");
    let flagged = lab.dir().join("t/flag.yml");

    let run = lab.run_from(&below, &["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    let calls = lab.calls();
    assert_eq!(
        calls[1],
        call(&[
            "docker",
            "ps",
            "-a",
            "--filter",
            "label=com.docker.compose.project=p",
            "--format",
            "{{.Names}}"
        ])
    );
    assert_eq!(
        calls[4],
        containerlab(&lab.dir().join("t/set.yml"), "deploy", &[])
    );
    assert_eq!(
        calls[5],
        docker_compose("p", &lab.compose("ch1"), &["up", "-d", "--remove-orphans"])
    );

    for given in ["t/flag.yml", flagged.to_str().unwrap()] {
        lab.script(&[]);
        let args = [
            "lab",
            "deploy",
            "-s",
            "ch1",
            "--wait",
            "0",
            "--topology",
            given,
        ];

        let run = lab.run_from(&below, &args, &[]);

        assert_eq!(run.code, 0, "{given}: {}", run.stderr);
        assert_eq!(
            lab.calls()[4],
            containerlab(&flagged, "deploy", &[]),
            "{given}"
        );
    }

    lab.script(&[(0, "clab-other-n1\n")]);
    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab deploy: node containers already exist (clab-other-n1); run nobs lab destroy first\n"
    );
}

#[test]
fn a_node_container_already_present_refuses_the_deploy_before_anything_else() {
    let lab = Lab::new("clash-node");
    lab.script(&[(0, "other\nclab-nobs-t-book-nobs-t-srl1\n")]);

    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab deploy: node containers already exist (clab-nobs-t-book-nobs-t-srl1); run nobs lab destroy first\n"
    );
    assert_eq!(run.stdout, "");
    assert_eq!(lab.calls(), [call(CONTAINERS)]);
}

#[test]
fn a_container_of_the_project_refuses_the_deploy_after_two_listings() {
    let lab = Lab::new("clash-project");
    lab.script(&[(0, ""), (0, "netobs-sleeper-1\nnetobs-other-1\n")]);

    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab deploy: project netobs already has containers (netobs-sleeper-1, netobs-other-1); run nobs lab destroy -s ch1 first, or nobs lab purge\n"
    );
    assert_eq!(run.stdout, "");
    assert_eq!(lab.calls(), [call(CONTAINERS), call(PROJECT_CONTAINERS)]);
}

#[test]
fn an_existing_network_is_kept_and_not_created() {
    let lab = Lab::new("network-kept");
    lab.script(&[(0, ""), (0, ""), (0, "bridge\nnetwork-observability\n")]);

    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            call(CONTAINERS),
            call(PROJECT_CONTAINERS),
            call(NETWORKS),
            containerlab(&lab.topology(), "deploy", &[]),
            docker_compose(
                "netobs",
                &lab.compose("ch1"),
                &["up", "-d", "--remove-orphans"]
            ),
        ]
    );
    assert_eq!(
        run.stdout,
        format!(
            "lab deploy: network network-observability exists, kept\n\
             lab deploy: deploying topology {}\n\
             lab deploy: starting ch1 (project netobs)\n\
             lab deployed: scenario ch1\n",
            lab.topology().display()
        )
    );
}

#[test]
fn a_listing_that_fails_stops_the_deploy_at_once() {
    let lab = Lab::new("listing-fails");
    let whats = ["containers", "containers of project netobs", "networks"];

    for (index, what) in whats.into_iter().enumerate() {
        let mut steps = vec![(0, ""); index];
        steps.push((4, ""));
        lab.script(&steps);

        let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);

        assert_eq!(run.code, 1, "{what}");
        assert_eq!(
            run.stderr,
            format!("nobs: lab deploy: cannot list {what}: exit 4\n")
        );
        assert_eq!(lab.calls().len(), index + 1, "{what}");
        assert_eq!(run.stdout, "", "{what}");
    }
}

#[test]
fn a_program_that_cannot_start_stops_the_deploy_with_exit_1() {
    let lab = Lab::new("no-docker");
    lab.without("docker");
    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);
    assert_eq!(run.code, 1);
    assert!(
        run.stderr
            .starts_with("nobs: lab deploy: cannot list containers: cannot run docker: "),
        "{}",
        run.stderr
    );

    let lab = Lab::new("no-containerlab");
    lab.without("containerlab");
    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);
    assert_eq!(run.code, 1);
    assert!(
        run.stderr
            .starts_with("nobs: lab deploy: containerlab deploy failed: cannot run containerlab: "),
        "{}",
        run.stderr
    );
    assert_eq!(lab.calls().len(), 4);
    assert!(!run.stdout.contains("lab deployed"), "{}", run.stdout);
}

#[test]
fn a_step_that_fails_stops_the_deploy_with_its_name_and_its_code() {
    let lab = Lab::new("step-fails");

    for (index, step) in [
        (3, "network create"),
        (4, "containerlab deploy"),
        (5, "compose up"),
    ] {
        lab.script(&ready(&[(index, 3, "")]));

        let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);

        assert_eq!(run.code, 3, "{step}");
        assert_eq!(
            run.stderr,
            format!("nobs: lab deploy: {step} failed (exit 3)\n")
        );
        assert_eq!(lab.calls().len(), index + 1, "{step}");
        assert!(
            !run.stdout.contains("lab deployed"),
            "{step}: {}",
            run.stdout
        );
    }
}

#[test]
fn a_service_that_exited_keeps_the_wait_polling_until_the_limit() {
    let lab = Lab::new("wait-limit");
    let mut steps = ready(&[(6, 0, SERVICE_EXITED), (7, 0, NODE_ONE_UP)]);
    steps.extend([(0, SERVICE_EXITED), (0, NODE_ONE_UP)]);
    lab.script(&steps);

    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "1"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab deploy: not ready after 1s: sleeper (exited/), clab-nobs-t-book-nobs-t-srl2 (missing)\n"
    );
    assert_eq!(lab.calls().len(), 10);
    assert!(run
        .stdout
        .contains("lab deploy: waiting for 1 services and 2 nodes (up to 1s)\n"));
    assert!(!run.stdout.contains("ready after"), "{}", run.stdout);
    assert!(!run.stdout.contains("lab deployed"), "{}", run.stdout);
}

#[test]
fn a_poll_that_fails_or_cannot_be_read_stops_the_wait_at_once() {
    let lab = Lab::new("poll-fails");
    let cases: [(usize, i32, &str, &str, i32); 3] = [
        (
            6,
            4,
            "",
            "nobs: lab deploy: compose ps failed (exit 4)\n",
            4,
        ),
        (
            7,
            5,
            "",
            "nobs: lab deploy: containerlab inspect failed (exit 5)\n",
            5,
        ),
        (
            6,
            0,
            "not json",
            "nobs: lab deploy: cannot read compose ps output: ",
            1,
        ),
    ];

    for (index, code, text, line, exit) in cases {
        lab.script(&ready(&[(index, code, text)]));

        let run = lab.run(&["lab", "deploy", "-s", "ch1"], &[]);

        assert_eq!(run.code, exit, "{line}");
        assert!(run.stderr.starts_with(line), "{}", run.stderr);
        assert_eq!(lab.calls().len(), index + 1, "{line}");
        assert!(
            !run.stdout.contains("ready after"),
            "{line}: {}",
            run.stdout
        );
        assert!(
            !run.stdout.contains("lab deployed"),
            "{line}: {}",
            run.stdout
        );
    }
}

#[test]
fn wait_0_makes_no_poll_and_still_says_deployed() {
    let lab = Lab::new("wait-zero");

    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(lab.calls().len(), 6);
    assert!(!run.stdout.contains("waiting for"), "{}", run.stdout);
    assert!(
        run.stdout.ends_with("lab deployed: scenario ch1\n"),
        "{}",
        run.stdout
    );
}

#[test]
fn destroy_takes_the_stack_down_with_its_volumes_then_the_topology_and_runs_no_network_command() {
    let lab = Lab::new("destroy");
    let (topology, ch1) = (lab.topology(), lab.compose("ch1"));

    let run = lab.run(&["lab", "destroy", "-s", "ch1"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            docker_compose("netobs", &ch1, &["down", "--volumes", "--remove-orphans"]),
            containerlab(&topology, "destroy", &["--cleanup"]),
        ]
    );
    assert_eq!(
        run.stdout,
        format!(
            "lab destroy: stopping ch1 (project netobs)\n\
             lab destroy: destroying topology {}\n\
             lab destroyed: scenario ch1\n",
            topology.display()
        )
    );
}

#[test]
fn destroy_stops_at_the_first_failing_step_with_its_code() {
    let lab = Lab::new("destroy-fails");

    for (index, step, code) in [(0, "compose down", 3), (1, "containerlab destroy", 4)] {
        let mut steps = vec![(0, ""); index];
        steps.push((code, ""));
        lab.script(&steps);

        let run = lab.run(&["lab", "destroy", "-s", "ch1"], &[]);

        assert_eq!(run.code, code, "{step}");
        assert_eq!(
            run.stderr,
            format!("nobs: lab destroy: {step} failed (exit {code})\n")
        );
        assert_eq!(lab.calls().len(), index + 1, "{step}");
        assert!(
            !run.stdout.contains("lab destroyed"),
            "{step}: {}",
            run.stdout
        );
    }
}

fn purge_lab(name: &str) -> Lab {
    let lab = Lab::new(name);
    lab.file("chapters/ch2/docker-compose.yml", "services: {}\n");
    lab.file("chapters/ch3/docker-compose.yml", "services: {}\n");
    lab.file("chapters/notes/readme.md", "");
    lab
}

fn purge_downs(lab: &Lab) -> Vec<Call> {
    ["ch1", "ch2", "ch3"]
        .iter()
        .map(|scenario| {
            docker_compose(
                "netobs",
                &lab.compose(scenario),
                &["down", "--volumes", "--remove-orphans"],
            )
        })
        .collect()
}

#[test]
fn purge_goes_on_after_a_failing_down_and_destroys_the_topology_once() {
    let lab = purge_lab("purge-fails");
    lab.script(&[
        (0, ""),
        (3, ""),
        (4, ""),
        (0, "other\nclab-nobs-t-book-nobs-t-srl1\n"),
    ]);

    let run = lab.run(&["lab", "purge"], &[]);

    assert_eq!(run.code, 3);
    let mut expected = purge_downs(&lab);
    expected.push(call(CONTAINERS));
    expected.push(containerlab(&lab.topology(), "destroy", &["--cleanup"]));
    assert_eq!(lab.calls(), expected);
    assert_eq!(
        run.stderr,
        "nobs: lab purge: compose down failed (exit 3)\nnobs: lab purge: compose down failed (exit 4)\n"
    );
    assert_eq!(
        run.stdout,
        format!(
            "lab purge: ch1\nlab purge: ch2\nlab purge: ch3\nlab purge: destroying topology {}\n",
            lab.topology().display()
        )
    );
}

#[test]
fn purge_without_a_node_present_says_so_and_then_says_purged() {
    let lab = purge_lab("purge-clean");

    let run = lab.run(&["lab", "purge"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    let mut expected = purge_downs(&lab);
    expected.push(call(CONTAINERS));
    assert_eq!(lab.calls(), expected);
    assert_eq!(
        run.stdout,
        "lab purge: ch1\nlab purge: ch2\nlab purge: ch3\nlab purge: no lab nodes to destroy\nlab purged: 3 scenarios\n"
    );
}

#[test]
fn purge_with_a_failing_node_listing_exits_1_and_neither_destroys_nor_says_purged() {
    let lab = purge_lab("purge-listing");
    lab.script(&[(0, ""), (0, ""), (0, ""), (5, "")]);

    let run = lab.run(&["lab", "purge"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab purge: cannot list containers: exit 5\n"
    );
    assert_eq!(lab.calls().len(), 4);
    assert!(!run.stdout.contains("no lab nodes"), "{}", run.stdout);
    assert!(!run.stdout.contains("lab purged"), "{}", run.stdout);
}

#[test]
fn show_lists_the_stack_then_the_nodes_and_prints_nothing_itself() {
    let lab = Lab::new("show");

    let run = lab.run(&["lab", "show", "-s", "ch1"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            docker_compose("netobs", &lab.compose("ch1"), &["ps", "--all"]),
            containerlab(&lab.topology(), "inspect", &[]),
        ]
    );
    assert_eq!(run.stdout, "");
}

#[test]
fn show_passes_on_the_code_of_a_failing_step_and_skips_what_follows() {
    let lab = Lab::new("show-fails");

    for (index, step, code) in [(0, "compose ps", 4), (1, "containerlab inspect", 5)] {
        let mut steps = vec![(0, ""); index];
        steps.push((code, ""));
        lab.script(&steps);

        let run = lab.run(&["lab", "show", "-s", "ch1"], &[]);

        assert_eq!(run.code, code, "{step}");
        assert_eq!(
            run.stderr,
            format!("nobs: lab show: {step} failed (exit {code})\n")
        );
        assert_eq!(lab.calls().len(), index + 1, "{step}");
    }
}

#[test]
fn no_scenario_at_all_exits_2_and_names_the_known_ones() {
    let lab = Lab::new("no-scenario");
    lab.file("chapters/ch2/docker-compose.yml", "services: {}\n");

    for command in ["deploy", "destroy", "show"] {
        let run = lab.run(&["lab", command], &[]);

        assert_eq!(run.code, 2, "{command}");
        assert_eq!(
            run.stderr,
            format!("nobs: lab {command} needs --scenario NAME or LAB_SCENARIO\nknown: ch1, ch2\n")
        );
    }
    assert!(lab.calls().is_empty());
}

#[test]
fn an_unknown_scenario_exits_1_and_names_the_known_ones() {
    let lab = Lab::new("unknown-scenario");
    lab.file("chapters/ch2/docker-compose.yml", "services: {}\n");

    let run = lab.run(&["lab", "deploy", "-s", "nope"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        format!(
            "nobs: no scenario named nope under {}/chapters\nknown: ch1, ch2\n",
            lab.dir().display()
        )
    );
    assert!(lab.calls().is_empty());
}

#[test]
fn a_lab_sudo_that_is_not_a_truth_word_exits_2_before_anything_runs() {
    let lab = Lab::new("bad-sudo");

    let run = lab.run(&["lab", "deploy", "-s", "ch1"], &[("LAB_SUDO", "maybe")]);

    assert_eq!(run.code, 2);
    assert_eq!(
        run.stderr,
        "nobs: LAB_SUDO must be true or false, not maybe\n"
    );
    assert!(lab.calls().is_empty());
}

#[test]
fn a_workshop_and_a_labcli_lab_are_refused_with_exit_1_and_nothing_runs() {
    let lab = Lab::new("wrong-kind");

    for (folder, kind) in [
        ("workshop-one", "a workshop"),
        ("labcli-example", "a labcli lab"),
    ] {
        let dir = fixture(folder);

        let run = lab.run_from(&dir, &["lab", "deploy", "-s", "ch1"], &[]);

        assert_eq!(run.code, 1, "{folder}");
        assert_eq!(
            run.stderr,
            format!(
                "nobs: lab commands need the book lab (chapters/ and containerlab/lab.yml); {} is {kind}\n",
                dir.display()
            )
        );
    }
    assert!(lab.calls().is_empty());
}

#[test]
fn a_missing_topology_stops_every_command_before_anything_runs() {
    let lab = Lab::new("missing-topology");
    let missing = lab.dir().join("t/missing.yml");
    let commands: [&[&str]; 4] = [
        &["deploy", "-s", "ch1"],
        &["destroy", "-s", "ch1"],
        &["show", "-s", "ch1"],
        &["purge"],
    ];

    for command in commands {
        let mut args = vec!["lab"];
        args.extend(command);
        args.extend(["--topology", "t/missing.yml"]);

        let run = lab.run(&args, &[]);

        assert_eq!(run.code, 1, "{command:?}");
        assert_eq!(
            run.stderr,
            format!("nobs: topology file not found: {}\n", missing.display())
        );
    }
    assert!(lab.calls().is_empty());
}

#[test]
fn a_topology_that_is_not_a_lab_stops_deploy_and_purge_but_not_destroy() {
    let lab = Lab::new("bad-topology");
    lab.file("t/bad.yml", "mgmt: {}\n");
    let bad = lab.dir().join("t/bad.yml");

    for command in [&["deploy", "-s", "ch1"][..], &["purge"][..]] {
        let mut args = vec!["lab"];
        args.extend(command);
        args.extend(["--topology", "t/bad.yml"]);

        let run = lab.run(&args, &[]);

        assert_eq!(run.code, 1, "{command:?}");
        assert!(
            run.stderr.starts_with(&format!(
                "nobs: cannot read topology file: {}: ",
                bad.display()
            )),
            "{}",
            run.stderr
        );
    }
    assert!(lab.calls().is_empty());

    let run = lab.run(
        &["lab", "destroy", "-s", "ch1", "--topology", "t/bad.yml"],
        &[],
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(lab.calls().len(), 2);
}

#[test]
fn the_deploy_help_names_every_flag_and_the_default_wait() {
    let lab = Lab::new("help");

    let run = lab.run(&["lab", "deploy", "--help"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    for flag in [
        "--scenario",
        "--topology",
        "--network-name",
        "--subnet",
        "--sudo",
        "--no-sudo",
        "--wait",
        "[default: 300]",
    ] {
        assert!(run.stdout.contains(flag), "{flag}\n{}", run.stdout);
    }
}
