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
const BUILDER_UP: &str = r#"{"Service":"builder","State":"running","Health":""}"#;
const SERVICES_MIXED: &str = concat!(
    r#"{"Service":"sleeper","State":"running","Health":""}"#,
    "\n",
    r#"{"Service":"other","State":"exited","Health":""}"#
);
const SRLINUX: &str = "ghcr.io/nokia/srlinux:latest";

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

fn inspect(image: &str) -> Call {
    call(&["docker", "image", "inspect", "--format", "{{.Id}}", image])
}
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
    let mut steps = vec![(0, ""); 7];
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
            inspect(SRLINUX),
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
        assert_eq!(&lab.calls()[5], expected, "{flags:?} {env:?}");
    }

    lab.script(&ready(&[]));
    lab.run(&["lab", "deploy", "-s", "ch1", "--sudo"], &[]);
    assert_eq!(
        lab.calls()[8],
        sudo_containerlab(&topology, "inspect", &["--format", "json"])
    );
}

#[test]
fn sudo_reaches_the_containerlab_calls_of_destroy_show_and_purge() {
    let lab = Lab::new("sudo-others");
    let (topology, ch1) = (lab.topology(), lab.compose("ch1"));
    let down = docker_compose("netobs", &ch1, &["down", "--volumes", "--remove-orphans"]);
    let node = vec![(0, ""), (0, "clab-nobs-t-book-nobs-t-srl1\n")];
    let cases: [(Flags, Env, Vec<Step>, Vec<Call>); 4] = [
        (
            &["destroy", "-s", "ch1", "--sudo"],
            &[],
            vec![],
            vec![
                down.clone(),
                sudo_containerlab(&topology, "destroy", &["--cleanup"]),
                call(NETWORKS),
            ],
        ),
        (
            &["show", "-s", "ch1", "--sudo"],
            &[],
            vec![],
            vec![
                docker_compose("netobs", &ch1, &["ps", "--all"]),
                sudo_containerlab(&topology, "inspect", &[]),
            ],
        ),
        (
            &["purge", "--sudo"],
            &[],
            node.clone(),
            vec![
                down.clone(),
                call(CONTAINERS),
                sudo_containerlab(&topology, "destroy", &["--cleanup"]),
                call(NETWORKS),
            ],
        ),
        (
            &["purge", "--no-sudo"],
            &[("LAB_SUDO", "true")],
            node,
            vec![
                down,
                call(CONTAINERS),
                containerlab(&topology, "destroy", &["--cleanup"]),
                call(NETWORKS),
            ],
        ),
    ];

    for (command, env, steps, expected) in cases {
        lab.script(&steps);
        let mut args = vec!["lab"];
        args.extend(command);

        let run = lab.run(&args, env);

        assert_eq!(run.code, 0, "{command:?} {env:?}: {}", run.stderr);
        assert_eq!(lab.calls(), expected, "{command:?} {env:?}");
    }
}

#[test]
fn sudo_no_prompt_puts_n_first_in_every_sudo_call_of_destroy_and_deploy() {
    let lab = Lab::new("sudo-n");
    let mut deploy = vec![(0, "")];
    deploy.extend(ready(&[]));
    let cases: [(Flags, Vec<Step>, usize); 2] = [
        (&["destroy", "-s", "ch1"], vec![], 2),
        (&["deploy", "-s", "ch1"], deploy, 3),
    ];

    for (command, steps, count) in cases {
        lab.script(&steps);
        let mut args = vec!["lab"];
        args.extend(command);
        args.extend(["--sudo", "--sudo-no-prompt"]);

        let run = lab.run(&args, &[]);

        assert_eq!(run.code, 0, "{command:?}: {}", run.stderr);
        let sudo: Vec<Call> = lab
            .calls()
            .into_iter()
            .filter(|call| call[0] == "sudo")
            .collect();
        assert_eq!(sudo.len(), count, "{command:?}: {sudo:?}");
        assert!(
            sudo.iter().all(|call| call[1] == "-n"),
            "{command:?}: {sudo:?}"
        );
    }
}

#[test]
fn a_sudo_that_needs_a_password_stops_destroy_and_deploy_before_docker_with_exit_1() {
    let lab = Lab::new("sudo-password");
    let sudo = lab.bin().join("sudo");
    fs::write(
        &sudo,
        "#!/bin/sh\nif [ \"$1\" = \"-n\" ]; then\n  echo 'sudo: a password is required' >&2\n  exit 1\nfi\n: > \"${0%/*}/marker\"\nexit 97\n",
    )
    .unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o755)).unwrap();

    for command in ["destroy", "deploy"] {
        lab.script(&[]);

        let run = lab.run(
            &["lab", command, "-s", "ch1", "--sudo", "--sudo-no-prompt"],
            &[],
        );

        assert_eq!(run.code, 1, "{command}: {}", run.stderr);
        assert!(lab.calls().is_empty(), "{command}: {:?}", lab.calls());
        assert!(!lab.bin().join("marker").exists(), "{command}");
        assert_eq!(
            run.stderr,
            format!(
                "sudo: a password is required\n\
                 sudo: a password is required\n\
                 nobs: lab {command}: sudo needs a password and cannot ask for it here; run sudo -v in this terminal, then try again\n"
            )
        );
    }
}

#[test]
fn a_containerlab_step_that_fails_after_a_passed_check_reports_its_own_exit() {
    let lab = Lab::new("sudo-step-fails");
    lab.script(&[(0, ""), (0, ""), (3, "")]);

    let run = lab.run(
        &["lab", "destroy", "-s", "ch1", "--sudo", "--sudo-no-prompt"],
        &[],
    );

    assert_eq!(run.code, 3);
    assert_eq!(
        run.stderr,
        "nobs: lab destroy: containerlab destroy failed (exit 3)\n"
    );
}

#[test]
fn a_failed_check_with_a_sudo_that_needs_no_password_goes_on_with_destroy() {
    let lab = Lab::new("sudo-check-fails");
    lab.script(&[(1, ""), (0, "")]);
    let (topology, ch1) = (lab.topology(), lab.compose("ch1"));
    let mut destroy = sudo_containerlab(&topology, "destroy", &["--cleanup"]);
    destroy.insert(1, "-n".to_string());

    let run = lab.run(
        &["lab", "destroy", "-s", "ch1", "--sudo", "--sudo-no-prompt"],
        &[],
    );

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "");
    assert_eq!(
        lab.calls(),
        [
            call(&[
                "sudo",
                "-n",
                "--preserve-env=NOBS_T_LABEL",
                "containerlab",
                "version"
            ]),
            call(&["sudo", "-n", "true"]),
            docker_compose("netobs", &ch1, &["down", "--volumes", "--remove-orphans"]),
            destroy,
            call(NETWORKS),
        ]
    );
}

#[test]
fn sudo_no_prompt_without_sudo_changes_nothing() {
    let lab = Lab::new("no-sudo-n");
    let (topology, ch1) = (lab.topology(), lab.compose("ch1"));

    let run = lab.run(
        &[
            "lab",
            "destroy",
            "-s",
            "ch1",
            "--no-sudo",
            "--sudo-no-prompt",
        ],
        &[],
    );

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            docker_compose("netobs", &ch1, &["down", "--volumes", "--remove-orphans"]),
            containerlab(&topology, "destroy", &["--cleanup"]),
            call(NETWORKS),
        ]
    );
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
        assert_eq!(lab.calls()[4], expected, "{flags:?} {env:?}");
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
        assert_eq!(lab.calls()[6], up, "{flags:?} {env:?}");
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
    lab.script(&[
        (0, ""),
        (0, ""),
        (0, ""),
        (0, "bridge\nnetwork-observability\n"),
    ]);

    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            call(CONTAINERS),
            call(PROJECT_CONTAINERS),
            inspect(SRLINUX),
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
    let whats = [
        (0, "containers"),
        (1, "containers of project netobs"),
        (3, "networks"),
    ];

    for (index, what) in whats {
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
    assert_eq!(lab.calls().len(), 5);
    assert!(!run.stdout.contains("lab deployed"), "{}", run.stdout);
}

#[test]
fn a_step_that_fails_stops_the_deploy_with_its_name_and_its_code() {
    let lab = Lab::new("step-fails");

    for (index, step) in [
        (4, "network create"),
        (5, "containerlab deploy"),
        (6, "compose up"),
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
    let mut steps = ready(&[(7, 0, SERVICE_EXITED), (8, 0, NODE_ONE_UP)]);
    steps.extend([(0, SERVICE_EXITED), (0, NODE_ONE_UP)]);
    lab.script(&steps);

    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "1"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab deploy: not ready after 1s: sleeper (exited/), clab-nobs-t-book-nobs-t-srl2 (missing)\n"
    );
    assert_eq!(lab.calls().len(), 11);
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
            7,
            4,
            "",
            "nobs: lab deploy: compose ps failed (exit 4)\n",
            4,
        ),
        (
            8,
            5,
            "",
            "nobs: lab deploy: containerlab inspect failed (exit 5)\n",
            5,
        ),
        (
            7,
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
    assert_eq!(lab.calls().len(), 7);
    assert!(!run.stdout.contains("waiting for"), "{}", run.stdout);
    assert!(
        run.stdout.ends_with("lab deployed: scenario ch1\n"),
        "{}",
        run.stdout
    );
}

#[test]
fn destroy_takes_the_stack_down_with_its_volumes_then_the_topology_and_leaves_a_missing_network_alone(
) {
    let lab = Lab::new("destroy");
    let (topology, ch1) = (lab.topology(), lab.compose("ch1"));

    let run = lab.run(&["lab", "destroy", "-s", "ch1"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            docker_compose("netobs", &ch1, &["down", "--volumes", "--remove-orphans"]),
            containerlab(&topology, "destroy", &["--cleanup"]),
            call(NETWORKS),
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

fn attached(network: &str) -> Call {
    call(&[
        "docker",
        "ps",
        "-a",
        "--filter",
        &format!("network={network}"),
        "--format",
        "{{.Names}}",
    ])
}

fn network_rm(network: &str) -> Call {
    call(&["docker", "network", "rm", network])
}

#[test]
fn destroy_removes_a_network_no_container_uses_keeps_one_in_use_and_stops_on_a_failed_rm() {
    let lab = Lab::new("destroy-network");
    let (topology, ch1) = (lab.topology(), lab.compose("ch1"));
    let lab_net = "network-observability";
    let removing = "lab destroy: removing network network-observability\n";
    let destroyed = "lab destroyed: scenario ch1\n";
    type Case<'a> = (Env<'a>, Vec<Step>, i32, Vec<Call>, String, &'a str);
    let cases: [Case; 4] = [
        (
            &[],
            vec![(0, ""), (0, ""), (0, "network-observability\n")],
            0,
            vec![attached(lab_net), network_rm(lab_net)],
            format!("{removing}{destroyed}"),
            "",
        ),
        (
            &[("LAB_NETWORK_NAME", "nobs-t-net")],
            vec![(0, ""), (0, ""), (0, "network-observability\nnobs-t-net\n")],
            0,
            vec![attached("nobs-t-net"), network_rm("nobs-t-net")],
            format!("lab destroy: removing network nobs-t-net\n{destroyed}"),
            "",
        ),
        (
            &[],
            vec![
                (0, ""),
                (0, ""),
                (0, "network-observability\n"),
                (0, "stopped-client\n"),
            ],
            0,
            vec![attached(lab_net)],
            format!(
                "lab destroy: network network-observability is still in use, kept\n{destroyed}"
            ),
            "",
        ),
        (
            &[],
            vec![
                (0, ""),
                (0, ""),
                (0, "network-observability\n"),
                (0, ""),
                (1, ""),
            ],
            1,
            vec![attached(lab_net), network_rm(lab_net)],
            removing.to_string(),
            "nobs: lab destroy: network rm failed (exit 1)\n",
        ),
    ];

    for (env, steps, code, tail, stdout, stderr) in cases {
        lab.script(&steps);

        let run = lab.run(&["lab", "destroy", "-s", "ch1"], env);

        assert_eq!(run.code, code, "{env:?} {steps:?}: {}", run.stderr);
        let mut expected = vec![
            docker_compose("netobs", &ch1, &["down", "--volumes", "--remove-orphans"]),
            containerlab(&topology, "destroy", &["--cleanup"]),
            call(NETWORKS),
        ];
        expected.extend(tail);
        assert_eq!(lab.calls(), expected, "{env:?} {steps:?}");
        assert_eq!(
            run.stdout,
            format!(
                "lab destroy: stopping ch1 (project netobs)\n\
                 lab destroy: destroying topology {}\n\
                 {stdout}",
                topology.display()
            ),
            "{env:?} {steps:?}"
        );
        assert_eq!(run.stderr, stderr, "{env:?} {steps:?}");
    }
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
fn purge_without_a_node_present_says_so_removes_the_empty_network_and_then_says_purged() {
    let lab = purge_lab("purge-clean");
    lab.script(&[
        (0, ""),
        (0, ""),
        (0, ""),
        (0, ""),
        (0, "network-observability\n"),
    ]);

    let run = lab.run(&["lab", "purge"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    let mut expected = purge_downs(&lab);
    expected.push(call(CONTAINERS));
    expected.push(call(NETWORKS));
    expected.push(attached("network-observability"));
    expected.push(network_rm("network-observability"));
    assert_eq!(lab.calls(), expected);
    assert_eq!(
        run.stdout,
        "lab purge: ch1\nlab purge: ch2\nlab purge: ch3\nlab purge: no lab nodes to destroy\nlab purge: removing network network-observability\nlab purged: 3 scenarios\n"
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

    for command in ["deploy", "destroy", "show", "prepare", "update", "rebuild"] {
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
    let commands: [&[&str]; 5] = [
        &["deploy", "-s", "ch1"],
        &["destroy", "-s", "ch1"],
        &["show", "-s", "ch1"],
        &["purge"],
        &["prepare", "-s", "ch1"],
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
    assert_eq!(lab.calls().len(), 3);
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
        "--sudo-no-prompt",
        "--wait",
        "[default: 300]",
    ] {
        assert!(run.stdout.contains(flag), "{flag}\n{}", run.stdout);
    }
}

fn ch2_lab(name: &str) -> Lab {
    let lab = Lab::new(name);
    let book = fixture("book-lab");
    for file in [
        "chapters/ch2/docker-compose.yml",
        "chapters/ch2/builder/Dockerfile",
    ] {
        lab.file(file, &fs::read_to_string(book.join(file)).unwrap());
    }
    lab
}

fn ceos_topology(kind: &str, image: &str) -> String {
    format!(
        "name: nobs-t-ceos\ntopology:\n  nodes:\n    ceos1:\n      kind: {kind}\n      image: {image}\n"
    )
}

const CEOS_IMPORT: &str = "  Arista cEOS is not on a registry: download cEOS64-lab-<version>.tar.xz (x86_64) or cEOSarm-lab-<version>.tar.xz (arm64, the file for clab) from arista.com (an account is needed), then run: docker import <file>";

#[test]
fn a_missing_image_stops_the_deploy_after_the_two_listings_and_says_how_to_get_it() {
    let lab = Lab::new("image-missing");
    lab.script(&[(0, ""), (0, ""), (1, "")]);

    let run = lab.run(&["lab", "deploy", "-s", "ch1", "--wait", "0"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab deploy: image ghcr.io/nokia/srlinux:latest of nodes nobs-t-srl1, nobs-t-srl2 is not present\n  pull it with: docker pull ghcr.io/nokia/srlinux:latest\n"
    );
    assert_eq!(run.stdout, "");
    assert_eq!(
        lab.calls(),
        [call(CONTAINERS), call(PROJECT_CONTAINERS), inspect(SRLINUX)]
    );
}

#[test]
fn a_missing_ceos_image_gets_the_import_line_and_the_variable_to_set() {
    let lab = Lab::new("image-ceos");
    lab.file(
        "t/ceos.yml",
        &ceos_topology("ceos", "\"${NOBS_T_CEOS:-ceos:image}\""),
    );
    lab.script(&[(0, ""), (0, ""), (1, "")]);

    let run = lab.run(
        &[
            "lab",
            "deploy",
            "-s",
            "ch1",
            "--wait",
            "0",
            "--topology",
            "t/ceos.yml",
        ],
        &[],
    );

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        format!(
            "nobs: lab deploy: image ceos:image of node ceos1 is not present\n{CEOS_IMPORT} ceos:<version>\n  then set NOBS_T_CEOS=ceos:<version>\n"
        )
    );
    assert_eq!(lab.calls()[2], inspect("ceos:image"));
}

#[test]
fn a_ceos_image_without_a_variable_gets_the_import_line_for_that_image_only() {
    let lab = Lab::new("image-ceos-fixed");
    lab.file("t/ceos.yml", &ceos_topology("arista_ceos", "ceos:fixed"));
    lab.script(&[(0, ""), (0, ""), (1, "")]);

    let run = lab.run(
        &[
            "lab",
            "deploy",
            "-s",
            "ch1",
            "--wait",
            "0",
            "--topology",
            "t/ceos.yml",
        ],
        &[],
    );

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        format!(
            "nobs: lab deploy: image ceos:fixed of node ceos1 is not present\n{CEOS_IMPORT} ceos:fixed\n"
        )
    );
}

#[test]
fn a_set_variable_gives_the_image_that_is_inspected_and_a_present_image_lets_the_deploy_go_on() {
    let lab = Lab::new("image-variable");
    lab.file(
        "t/ceos.yml",
        &ceos_topology("ceos", "\"${NOBS_T_CEOS:-ceos:image}\""),
    );

    let run = lab.run(
        &[
            "lab",
            "deploy",
            "-s",
            "ch1",
            "--wait",
            "0",
            "--topology",
            "t/ceos.yml",
        ],
        &[("NOBS_T_CEOS", "ceos:4.32.0F")],
    );

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(lab.calls().len(), 7);
    assert_eq!(lab.calls()[2], inspect("ceos:4.32.0F"));
}

#[test]
fn a_node_without_an_image_is_not_checked_and_the_deploy_goes_on_to_the_network_step() {
    let lab = Lab::new("image-none");
    lab.file("t/none.yml", OTHER_TOPOLOGY);

    let run = lab.run(
        &[
            "lab",
            "deploy",
            "-s",
            "ch1",
            "--wait",
            "0",
            "--topology",
            "t/none.yml",
        ],
        &[],
    );

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            call(CONTAINERS),
            call(PROJECT_CONTAINERS),
            call(NETWORKS),
            call(CREATE),
            containerlab(&lab.dir().join("t/none.yml"), "deploy", &[]),
            docker_compose(
                "netobs",
                &lab.compose("ch1"),
                &["up", "-d", "--remove-orphans"]
            ),
        ]
    );
    assert_eq!(run.stderr, "");
}

#[test]
fn a_ceos_image_from_a_set_variable_gets_the_import_line_for_that_image_and_no_set_line() {
    let lab = Lab::new("image-ceos-set");
    lab.file(
        "t/ceos.yml",
        &ceos_topology("ceos", "\"${NOBS_T_CEOS:-ceos:image}\""),
    );
    lab.script(&[(0, ""), (0, ""), (1, "")]);

    let run = lab.run(
        &[
            "lab",
            "deploy",
            "-s",
            "ch1",
            "--wait",
            "0",
            "--topology",
            "t/ceos.yml",
        ],
        &[("NOBS_T_CEOS", "ceos:4.32.0F")],
    );

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        format!(
            "nobs: lab deploy: image ceos:4.32.0F of node ceos1 is not present\n{CEOS_IMPORT} ceos:4.32.0F\n"
        )
    );
    assert_eq!(lab.calls()[2], inspect("ceos:4.32.0F"));
}

#[test]
fn a_ceos_image_from_an_empty_variable_gets_the_import_line_and_the_variable_to_set() {
    let lab = Lab::new("image-ceos-empty");
    lab.file(
        "t/ceos.yml",
        &ceos_topology("ceos", "\"${NOBS_T_CEOS:-ceos:image}\""),
    );
    lab.script(&[(0, ""), (0, ""), (1, "")]);

    let run = lab.run(
        &[
            "lab",
            "deploy",
            "-s",
            "ch1",
            "--wait",
            "0",
            "--topology",
            "t/ceos.yml",
        ],
        &[("NOBS_T_CEOS", "")],
    );

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        format!(
            "nobs: lab deploy: image ceos:image of node ceos1 is not present\n{CEOS_IMPORT} ceos:<version>\n  then set NOBS_T_CEOS=ceos:<version>\n"
        )
    );
    assert_eq!(lab.calls()[2], inspect("ceos:image"));
}

#[test]
fn update_removes_the_named_service_with_its_volumes_starts_it_and_waits_for_it_alone() {
    let lab = Lab::new("update");
    lab.script(&[(0, ""), (0, ""), (0, SERVICES_MIXED)]);
    let ch1 = lab.compose("ch1");

    let run = lab.run(&["lab", "update", "sleeper", "-s", "ch1"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            docker_compose(
                "netobs",
                &ch1,
                &["rm", "--stop", "--force", "--volumes", "sleeper"]
            ),
            docker_compose("netobs", &ch1, &["up", "-d", "--remove-orphans", "sleeper"]),
            docker_compose("netobs", &ch1, &["ps", "--all", "--format", "json"]),
        ]
    );
    assert_eq!(
        run.stdout,
        "lab update: removing sleeper (project netobs)\n\
         lab update: starting sleeper\n\
         lab update: waiting for 1 services (up to 300s)\n\
         lab update: ready after 0s\n\
         lab updated: scenario ch1\n"
    );
    assert_eq!(run.stderr, "");
}

#[test]
fn update_without_a_service_acts_on_all_services_and_waits_for_the_whole_list() {
    let lab = Lab::new("update-all");
    lab.script(&[(0, ""), (0, ""), (0, SERVICE_UP)]);
    let ch1 = lab.compose("ch1");

    let run = lab.run(&["lab", "update", "-s", "ch1"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            docker_compose("netobs", &ch1, &["rm", "--stop", "--force", "--volumes"]),
            docker_compose("netobs", &ch1, &["up", "-d", "--remove-orphans"]),
            docker_compose("netobs", &ch1, &["ps", "--all", "--format", "json"]),
        ]
    );
    assert_eq!(
        run.stdout,
        "lab update: removing all services (project netobs)\n\
         lab update: starting all services\n\
         lab update: waiting for 1 services (up to 300s)\n\
         lab update: ready after 0s\n\
         lab updated: scenario ch1\n"
    );
}

#[test]
fn update_keeps_the_order_of_the_services_and_wait_0_makes_no_poll() {
    let lab = Lab::new("update-order");
    let ch1 = lab.compose("ch1");

    let run = lab.run(
        &["lab", "update", "zeta", "alpha", "-s", "ch1", "--wait", "0"],
        &[],
    );

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            docker_compose(
                "netobs",
                &ch1,
                &["rm", "--stop", "--force", "--volumes", "zeta", "alpha"]
            ),
            docker_compose(
                "netobs",
                &ch1,
                &["up", "-d", "--remove-orphans", "zeta", "alpha"]
            ),
        ]
    );
    assert_eq!(
        run.stdout,
        "lab update: removing zeta, alpha (project netobs)\n\
         lab update: starting zeta, alpha\n\
         lab updated: scenario ch1\n"
    );
}

#[test]
fn a_failing_rm_stops_the_update_with_its_code_and_no_success_line() {
    let lab = Lab::new("update-fails");
    lab.script(&[(2, "")]);

    let run = lab.run(
        &["lab", "update", "sleeper", "-s", "ch1", "--wait", "0"],
        &[],
    );

    assert_eq!(run.code, 2);
    assert_eq!(run.stderr, "nobs: lab update: compose rm failed (exit 2)\n");
    assert_eq!(lab.calls().len(), 1);
    assert_eq!(
        run.stdout,
        "lab update: removing sleeper (project netobs)\n"
    );
}

#[test]
fn update_and_rebuild_do_not_read_the_topology_file_but_deploy_does() {
    let lab = Lab::new("no-topology-read");
    lab.file(
        "nobs.toml",
        "kind = \"lab\"\ntopology = \"t/missing.yml\"\n",
    );

    for command in ["update", "rebuild"] {
        let run = lab.run(
            &["lab", command, "sleeper", "-s", "ch1", "--wait", "0"],
            &[],
        );

        assert_eq!(run.code, 0, "{command}: {}", run.stderr);
    }

    let run = lab.run(&["lab", "deploy", "-s", "ch1"], &[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        format!(
            "nobs: topology file not found: {}\n",
            lab.dir().join("t/missing.yml").display()
        )
    );
}

#[test]
fn rebuild_stops_builds_and_starts_the_service_then_waits_for_it() {
    let lab = ch2_lab("rebuild");
    lab.script(&[(0, ""), (0, ""), (0, ""), (0, BUILDER_UP)]);
    let ch2 = lab.compose("ch2");

    let run = lab.run(
        &["lab", "rebuild", "builder", "-s", "ch2", "--wait", "1"],
        &[],
    );

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            docker_compose("netobs", &ch2, &["stop", "builder"]),
            docker_compose("netobs", &ch2, &["build", "builder"]),
            docker_compose("netobs", &ch2, &["up", "-d", "--remove-orphans", "builder"]),
            docker_compose("netobs", &ch2, &["ps", "--all", "--format", "json"]),
        ]
    );
    assert_eq!(
        run.stdout,
        "lab rebuild: stopping builder\n\
         lab rebuild: building builder\n\
         lab rebuild: starting builder\n\
         lab rebuild: waiting for 1 services (up to 1s)\n\
         lab rebuild: ready after 0s\n\
         lab rebuilt: scenario ch2\n"
    );
    assert_eq!(run.stderr, "");
}

#[test]
fn a_failing_build_stops_the_rebuild_with_its_code_and_no_success_line() {
    let lab = ch2_lab("rebuild-fails");
    lab.script(&[(0, ""), (3, "")]);

    let run = lab.run(
        &["lab", "rebuild", "builder", "-s", "ch2", "--wait", "0"],
        &[],
    );

    assert_eq!(run.code, 3);
    assert_eq!(
        run.stderr,
        "nobs: lab rebuild: compose build failed (exit 3)\n"
    );
    assert_eq!(lab.calls().len(), 2);
    assert!(!run.stdout.contains("lab rebuilt"), "{}", run.stdout);
}

#[test]
fn a_service_that_stays_exited_keeps_the_services_wait_polling_until_the_limit() {
    let lab = Lab::new("services-limit");
    lab.script(&[
        (0, ""),
        (0, ""),
        (0, SERVICE_EXITED),
        (0, SERVICE_EXITED),
        (0, SERVICE_EXITED),
    ]);

    let run = lab.run(&["lab", "update", "-s", "ch1", "--wait", "1"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab update: not ready after 1s: sleeper (exited/)\n"
    );
    assert_eq!(lab.calls().len(), 4);
    assert!(run
        .stdout
        .contains("lab update: waiting for 1 services (up to 1s)\n"));
    assert!(!run.stdout.contains("ready after"), "{}", run.stdout);
    assert!(!run.stdout.contains("lab updated"), "{}", run.stdout);
}

#[test]
fn a_named_service_the_list_does_not_hold_is_missing_and_never_ready() {
    let lab = Lab::new("services-missing");
    lab.script(&[
        (0, ""),
        (0, ""),
        (0, SERVICE_UP),
        (0, SERVICE_UP),
        (0, SERVICE_UP),
    ]);

    let run = lab.run(
        &[
            "lab", "update", "sleeper", "ghost", "-s", "ch1", "--wait", "1",
        ],
        &[],
    );

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab update: not ready after 1s: ghost (missing)\n"
    );
    assert!(run
        .stdout
        .contains("lab update: waiting for 2 services (up to 1s)\n"));
    assert!(!run.stdout.contains("lab updated"), "{}", run.stdout);
}

#[test]
fn a_poll_that_fails_stops_the_services_wait_at_once() {
    let lab = Lab::new("services-poll-fails");
    lab.script(&[(0, ""), (0, ""), (0, ""), (4, "")]);

    let run = lab.run(&["lab", "rebuild", "-s", "ch1"], &[]);

    assert_eq!(run.code, 4);
    assert_eq!(
        run.stderr,
        "nobs: lab rebuild: compose ps failed (exit 4)\n"
    );
    assert_eq!(lab.calls().len(), 4);
    assert!(!run.stdout.contains("lab rebuilt"), "{}", run.stdout);
}

#[test]
fn prepare_checks_the_image_purges_every_scenario_then_deploys_and_says_so() {
    let lab = ch2_lab("prepare");
    let node = "clab-nobs-t-book-nobs-t-srl1\n";
    let mut steps = vec![(0, ""); 15];
    steps[0] = (0, node);
    steps[4] = (0, node);
    steps[13] = (0, BUILDER_UP);
    steps[14] = (0, NODES_UP);
    lab.script(&steps);
    let (topology, ch2) = (lab.topology(), lab.compose("ch2"));
    let down = |scenario: &str| {
        docker_compose(
            "netobs",
            &lab.compose(scenario),
            &["down", "--volumes", "--remove-orphans"],
        )
    };

    let run = lab.run(&["lab", "prepare", "-s", "ch2", "--wait", "1"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            call(CONTAINERS),
            inspect(SRLINUX),
            down("ch1"),
            down("ch2"),
            call(CONTAINERS),
            containerlab(&topology, "destroy", &["--cleanup"]),
            call(NETWORKS),
            call(CONTAINERS),
            call(PROJECT_CONTAINERS),
            call(NETWORKS),
            call(CREATE),
            containerlab(&topology, "deploy", &[]),
            docker_compose("netobs", &ch2, &["up", "-d", "--remove-orphans"]),
            docker_compose("netobs", &ch2, &["ps", "--all", "--format", "json"]),
            containerlab(&topology, "inspect", &["--format", "json"]),
        ]
    );
    assert_eq!(
        run.stdout,
        format!(
            "lab prepare: scenario ch2\n\
             lab purge: ch1\n\
             lab purge: ch2\n\
             lab purge: destroying topology {0}\n\
             lab purged: 2 scenarios\n\
             lab deploy: creating network network-observability (198.51.100.0/24)\n\
             lab deploy: deploying topology {0}\n\
             lab deploy: starting ch2 (project netobs)\n\
             lab deploy: waiting for 1 services and 2 nodes (up to 1s)\n\
             lab deploy: ready after 0s\n\
             lab deployed: scenario ch2\n\
             lab prepared: scenario ch2\n",
            topology.display()
        )
    );
    assert_eq!(run.stderr, "");
}

#[test]
fn prepare_purges_and_creates_the_network_its_network_name_flag_names() {
    let lab = Lab::new("prepare-network-name");
    let mut steps = vec![(0, ""); 13];
    steps[4] = (0, "network-observability\nnobs-t-net\n");
    lab.script(&steps);
    let (topology, ch1) = (lab.topology(), lab.compose("ch1"));

    let run = lab.run(
        &[
            "lab",
            "prepare",
            "-s",
            "ch1",
            "--network-name",
            "nobs-t-net",
            "--wait",
            "0",
        ],
        &[],
    );

    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        lab.calls(),
        [
            call(CONTAINERS),
            inspect(SRLINUX),
            docker_compose("netobs", &ch1, &["down", "--volumes", "--remove-orphans"]),
            call(CONTAINERS),
            call(NETWORKS),
            attached("nobs-t-net"),
            network_rm("nobs-t-net"),
            call(CONTAINERS),
            call(PROJECT_CONTAINERS),
            call(NETWORKS),
            call(&[
                "docker",
                "network",
                "create",
                "--driver=bridge",
                "--subnet=198.51.100.0/24",
                "nobs-t-net",
            ]),
            containerlab(&topology, "deploy", &[]),
            docker_compose("netobs", &ch1, &["up", "-d", "--remove-orphans"]),
        ]
    );
    assert!(run
        .stdout
        .contains("lab purge: removing network nobs-t-net\n"));
    assert!(run
        .stdout
        .contains("lab deploy: creating network nobs-t-net (198.51.100.0/24)\n"));
    assert_eq!(run.stderr, "");
}

#[test]
fn prepare_with_a_missing_image_refuses_before_any_down() {
    let lab = Lab::new("prepare-image");
    lab.script(&[(0, ""), (1, "")]);

    let run = lab.run(&["lab", "prepare", "-s", "ch1", "--wait", "0"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab prepare: image ghcr.io/nokia/srlinux:latest of nodes nobs-t-srl1, nobs-t-srl2 is not present\n  pull it with: docker pull ghcr.io/nokia/srlinux:latest\n"
    );
    assert_eq!(run.stdout, "");
    assert_eq!(lab.calls(), [call(CONTAINERS), inspect(SRLINUX)]);
}

#[test]
fn prepare_goes_on_after_a_failed_down_then_stops_with_its_code_and_does_not_deploy() {
    let lab = ch2_lab("prepare-purge-fails");
    lab.script(&[(0, ""), (0, ""), (1, "")]);
    let down = |scenario: &str| {
        docker_compose(
            "netobs",
            &lab.compose(scenario),
            &["down", "--volumes", "--remove-orphans"],
        )
    };

    let run = lab.run(&["lab", "prepare", "-s", "ch2", "--wait", "0"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        lab.calls(),
        [
            call(CONTAINERS),
            inspect(SRLINUX),
            down("ch1"),
            down("ch2"),
            call(CONTAINERS)
        ]
    );
    assert_eq!(
        run.stderr,
        "nobs: lab purge: compose down failed (exit 1)\n"
    );
    assert_eq!(
        run.stdout,
        "lab prepare: scenario ch2\nlab purge: ch1\nlab purge: ch2\nlab purge: no lab nodes to destroy\n"
    );
}

#[test]
fn prepare_with_a_failing_first_listing_exits_1_and_checks_no_image() {
    let lab = Lab::new("prepare-listing");
    lab.script(&[(5, "")]);

    let run = lab.run(&["lab", "prepare", "-s", "ch1", "--wait", "0"], &[]);

    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "nobs: lab prepare: cannot list containers: exit 5\n"
    );
    assert_eq!(run.stdout, "");
    assert_eq!(lab.calls(), [call(CONTAINERS)]);
}

#[test]
fn a_failing_deploy_step_in_prepare_is_named_deploy_and_there_is_no_prepared_line() {
    let lab = Lab::new("prepare-deploy-fails");
    let mut steps = vec![(0, ""); 10];
    steps[9] = (3, "");
    lab.script(&steps);

    let run = lab.run(&["lab", "prepare", "-s", "ch1", "--wait", "0"], &[]);

    assert_eq!(run.code, 3);
    assert_eq!(
        run.stderr,
        "nobs: lab deploy: containerlab deploy failed (exit 3)\n"
    );
    assert_eq!(lab.calls().len(), 10);
    assert!(!run.stdout.contains("lab prepared"), "{}", run.stdout);
}

#[test]
fn rebuild_passes_the_names_to_compose_unchecked_and_names_the_failing_step() {
    let lab = Lab::new("rebuild-nosuch");
    lab.script(&[(1, "")]);

    let run = lab.run(
        &["lab", "rebuild", "nosuch", "-s", "ch1", "--wait", "0"],
        &[],
    );

    assert_eq!(run.code, 1);
    assert_eq!(
        lab.calls(),
        [docker_compose(
            "netobs",
            &lab.compose("ch1"),
            &["stop", "nosuch"]
        )]
    );
    assert_eq!(
        run.stderr,
        "nobs: lab rebuild: compose stop failed (exit 1)\n"
    );
}

#[test]
fn prepare_update_and_rebuild_are_refused_in_a_workshop_folder() {
    let lab = Lab::new("new-commands-workshop");
    let dir = fixture("workshop-one");

    for command in ["prepare", "update", "rebuild"] {
        let run = lab.run_from(&dir, &["lab", command, "-s", "ch1"], &[]);

        assert_eq!(run.code, 1, "{command}");
        assert_eq!(
            run.stderr,
            format!(
                "nobs: lab commands need the book lab (chapters/ and containerlab/lab.yml); {} is a workshop\n",
                dir.display()
            ),
            "{command}"
        );
    }
    assert!(lab.calls().is_empty());
}

#[test]
fn the_update_and_rebuild_help_name_the_service_the_scenario_and_the_wait_but_no_topology() {
    let lab = Lab::new("help-service");

    for command in ["update", "rebuild"] {
        let run = lab.run(&["lab", command, "--help"], &[]);

        assert_eq!(run.code, 0, "{}", run.stderr);
        for word in ["SERVICE", "--scenario", "--wait", "[default: 300]"] {
            assert!(
                run.stdout.contains(word),
                "{command} {word}\n{}",
                run.stdout
            );
        }
        for word in ["--topology", "--sudo"] {
            assert!(
                !run.stdout.contains(word),
                "{command} {word}\n{}",
                run.stdout
            );
        }
    }
}

#[test]
fn the_prepare_help_names_every_flag_of_a_deploy_and_a_purge() {
    let lab = Lab::new("help-prepare");

    let run = lab.run(&["lab", "prepare", "--help"], &[]);

    assert_eq!(run.code, 0, "{}", run.stderr);
    for flag in [
        "--scenario",
        "--topology",
        "--network-name",
        "--subnet",
        "--no-sudo",
        "--sudo-no-prompt",
        "--wait",
        "[default: 300]",
    ] {
        assert!(run.stdout.contains(flag), "{flag}\n{}", run.stdout);
    }
    assert!(
        run.stdout
            .lines()
            .any(|line| line.trim_start().starts_with("--sudo")),
        "--sudo\n{}",
        run.stdout
    );
}
