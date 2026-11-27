use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../tests/support/stub.rs"]
mod stub;

#[path = "support/product_code.rs"]
mod product_code;

#[path = "support/tree.rs"]
mod tree;

use clap::CommandFactory;
use nobs_cli::{Cli, WorkshopCli};
use product_code::product_code;
use stub::Stub;
use tree::commands;

const NAMES: [&str; 8] = [
    "METRICS_URL",
    "PROMETHEUS_URL",
    "LOGS_URL",
    "LOKI_URL",
    "ALERTS_URL",
    "ALERTMANAGER_URL",
    "INFRAHUB_ADDRESS",
    "PREFECT_API_URL",
];

const KIND_WORDS: [&str; 12] = [
    "metrics-url",
    "prom-url",
    "logs-url",
    "loki-url",
    "alerts-url",
    "am-url",
    "alertmanager-url",
    "sot-url",
    "infrahub-url",
    "address",
    "flows-url",
    "prefect-url",
];

const NEW_FLAGS: [(&str, &str); 5] = [
    ("--metrics-url", "/-/ready"),
    ("--logs-url", "/ready"),
    ("--alerts-url", "/-/ready"),
    ("--sot-url", "/health"),
    ("--flows-url", "/api/health"),
];

const TOKEN: &str = "nobs-t-token";

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str, env: &str) -> Self {
        let root =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-setting-names-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture(), &root.join("nobs-t-one"));
        let copy = Self(root);
        fs::write(copy.dir().join(".env"), env).unwrap();
        copy
    }

    fn dir(&self) -> PathBuf {
        self.0.join("nobs-t-one")
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/workshop-one")
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

fn closed_port_url() -> String {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}")
}

fn nobs(copy: &Copy, args: &[String], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command.arg("nobs-t-one").args(args).current_dir(copy.dir());
    for name in NAMES {
        command.env_remove(name);
    }
    for name in [
        "INFRAHUB_API_TOKEN",
        "GRAFANA_URL",
        "SONDA_SERVER_URL",
        "WEBHOOK_URL",
        "NOBS_DIR",
    ] {
        command.env_remove(name);
    }
    command.envs(envs.iter().copied()).output().unwrap()
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|word| word.to_string()).collect()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn healthy_stub() -> Stub {
    Stub::start(&[
        ("GET", "/api/health", 200, ""),
        ("GET", "/-/ready", 200, ""),
        ("GET", "/ready", 200, ""),
        ("GET", "/health", 200, ""),
    ])
}

fn paths(stub: &Stub) -> Vec<String> {
    stub.requests()
        .into_iter()
        .map(|request| request.path)
        .collect()
}

fn status_args(except: &str, closed: &str) -> Vec<String> {
    let mut args = words(&["status", "--grafana-url", closed, "--sonda-url", closed]);
    for (flag, _) in NEW_FLAGS {
        if flag != except {
            args.extend(words(&[flag, closed]));
        }
    }
    args
}

enum Given {
    Flag(&'static str),
    Variable(&'static str),
}

#[test]
fn status_reaches_the_address_given_by_each_name() {
    let cases = [
        ("--metrics-url", Given::Flag("--metrics-url")),
        ("--metrics-url", Given::Flag("--prom-url")),
        ("--metrics-url", Given::Variable("METRICS_URL")),
        ("--metrics-url", Given::Variable("PROMETHEUS_URL")),
        ("--logs-url", Given::Flag("--logs-url")),
        ("--logs-url", Given::Flag("--loki-url")),
        ("--logs-url", Given::Variable("LOGS_URL")),
        ("--logs-url", Given::Variable("LOKI_URL")),
        ("--alerts-url", Given::Flag("--alerts-url")),
        ("--alerts-url", Given::Flag("--am-url")),
        ("--alerts-url", Given::Flag("--alertmanager-url")),
        ("--alerts-url", Given::Variable("ALERTS_URL")),
        ("--alerts-url", Given::Variable("ALERTMANAGER_URL")),
        ("--sot-url", Given::Flag("--sot-url")),
        ("--sot-url", Given::Flag("--infrahub-url")),
        ("--sot-url", Given::Flag("--address")),
        ("--flows-url", Given::Flag("--flows-url")),
        ("--flows-url", Given::Flag("--prefect-url")),
    ];
    let copy = Copy::new("each-name", "");
    let closed = closed_port_url();

    for (kind, given) in cases {
        let stub = healthy_stub();
        let mut args = status_args(kind, &closed);
        let (name, envs) = match given {
            Given::Flag(flag) => {
                args.extend(words(&[flag, stub.url()]));
                (flag, vec![])
            }
            Given::Variable(variable) => (variable, vec![(variable, stub.url())]),
        };

        let output = nobs(&copy, &args, &envs);

        let health = NEW_FLAGS.iter().find(|(flag, _)| *flag == kind).unwrap().1;
        assert_eq!(paths(&stub), [health], "{name}\n{}", stderr(&output));
    }
}

#[test]
fn status_takes_the_metrics_address_from_the_first_place_then_the_new_name() {
    type Case = (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
        Option<&'static str>,
    );
    let cases: [Case; 8] = [
        ("env-new", &[], "METRICS_URL={stub}\n", None),
        ("env-old", &[], "PROMETHEUS_URL={stub}\n", None),
        (
            "shell-old-env-new",
            &[("PROMETHEUS_URL", "{stub}")],
            "METRICS_URL={closed}\n",
            None,
        ),
        (
            "shell-new-env-old",
            &[("METRICS_URL", "{stub}")],
            "PROMETHEUS_URL={closed}\n",
            None,
        ),
        (
            "env-both",
            &[],
            "PROMETHEUS_URL={closed}\nMETRICS_URL={stub}\n",
            None,
        ),
        (
            "env-then-setup",
            &[],
            "PROMETHEUS_URL={stub}\n",
            Some("METRICS_URL={closed}\n"),
        ),
        (
            "env-blank-new",
            &[],
            "METRICS_URL= \nPROMETHEUS_URL={stub}\n",
            None,
        ),
        (
            "shell-blank-new",
            &[("METRICS_URL", " "), ("PROMETHEUS_URL", "{stub}")],
            "",
            None,
        ),
    ];
    let closed = closed_port_url();

    for (name, shell, env, setup) in cases {
        let stub = healthy_stub();
        let fill = |text: &str| {
            text.replace("{stub}", stub.url())
                .replace("{closed}", &closed)
        };
        let copy = Copy::new(&format!("places-{name}"), &fill(env));
        if let Some(setup) = setup {
            fs::write(copy.dir().join(".setup.env"), fill(setup)).unwrap();
        }
        let shell: Vec<(&str, String)> = shell
            .iter()
            .map(|(key, value)| (*key, fill(value)))
            .collect();
        let envs: Vec<(&str, &str)> = shell
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();

        let output = nobs(&copy, &status_args("--metrics-url", &closed), &envs);

        assert_eq!(paths(&stub), ["/-/ready"], "{name}\n{}", stderr(&output));
    }
}

#[test]
fn old_and_new_names_give_the_same_output() {
    let copy = Copy::new("same-output", "");
    let closed = closed_port_url();
    let stub = healthy_stub();
    let args = status_args("--metrics-url", &closed);
    let alerts = Stub::start(&[("GET", "/api/v2/alerts*", 200, "[]")]);

    let pairs = [
        (
            nobs(&copy, &args, &[("PROMETHEUS_URL", stub.url())]),
            nobs(&copy, &args, &[("METRICS_URL", stub.url())]),
        ),
        (
            nobs(&copy, &words(&["alerts", "--am-url", alerts.url()]), &[]),
            nobs(
                &copy,
                &words(&["alerts", "--alerts-url", alerts.url()]),
                &[],
            ),
        ),
    ];

    for (old, new) in pairs {
        assert_eq!(stdout(&old), stdout(&new));
        assert_eq!(stderr(&old), stderr(&new));
        assert_eq!(old.status.code(), new.status.code());
    }
    assert_eq!(stub.requests().len(), 2);
    assert_eq!(alerts.requests().len(), 2);
}

fn any_answer() -> Stub {
    Stub::start(&[
        ("GET", "/health", 500, ""),
        ("GET", "/api/healthcheck", 500, ""),
        ("GET", "*", 200, ""),
        ("POST", "*", 500, "boom"),
    ])
}

const ALERTS_WORDS: [&[&str]; 3] = [&["--alerts-url"], &["--am-url"], &["--alertmanager-url"]];
const SOT_WORDS: [&[&str]; 3] = [&["--sot-url"], &["--infrahub-url"], &["--address"]];

fn command_asks_each_word(name: &str, command: &[&str], runs: &[&[&str]]) {
    for (run, flags) in runs.iter().enumerate() {
        let copy = Copy::new(&format!("command-{name}-{run}"), "");
        let stubs: Vec<Stub> = flags.iter().map(|_| any_answer()).collect();
        let mut args = words(command);
        for (flag, stub) in flags.iter().zip(&stubs) {
            args.extend(words(&[flag, stub.url()]));
        }
        if flags
            .iter()
            .any(|flag| SOT_WORDS.iter().any(|words| words[0] == *flag))
        {
            args.extend(words(&["--token", TOKEN]));
        }

        let output = nobs(&copy, &args, &[]);

        for (flag, stub) in flags.iter().zip(&stubs) {
            assert!(
                !stub.requests().is_empty(),
                "{name} {flag}\n{}",
                stderr(&output)
            );
        }
    }
}

#[test]
fn alerts_asks_every_alerts_word() {
    command_asks_each_word("alerts", &["alerts"], &ALERTS_WORDS);
}

#[test]
fn silences_asks_every_alerts_word() {
    command_asks_each_word("silences", &["silences"], &ALERTS_WORDS);
}

#[test]
fn page_asks_every_alerts_word() {
    command_asks_each_word("page", &["page"], &ALERTS_WORDS);
}

#[test]
fn rca_asks_every_logs_word() {
    command_asks_each_word("rca", &["rca"], &[&["--logs-url"], &["--loki-url"]]);
}

#[test]
fn cycle_asks_every_alerts_logs_and_flows_word() {
    command_asks_each_word(
        "cycle",
        &["cycle", "srl1", "10.1.99.2"],
        &[
            &["--alerts-url", "--logs-url", "--flows-url"],
            &["--am-url", "--loki-url", "--prefect-url"],
            &["--alertmanager-url"],
        ],
    );
}

#[test]
fn evidence_asks_every_metrics_logs_and_sot_word() {
    command_asks_each_word(
        "evidence",
        &["evidence", "srl1", "10.1.99.2"],
        &[
            &["--metrics-url", "--logs-url", "--sot-url"],
            &["--prom-url", "--loki-url", "--infrahub-url"],
            &["--address"],
        ],
    );
}

#[test]
fn try_it_asks_every_metrics_logs_alerts_and_sot_word() {
    command_asks_each_word(
        "try-it",
        &["try-it"],
        &[
            &["--metrics-url", "--logs-url", "--alerts-url", "--sot-url"],
            &["--prom-url", "--loki-url", "--am-url", "--infrahub-url"],
            &[
                "--metrics-url",
                "--logs-url",
                "--alertmanager-url",
                "--address",
            ],
        ],
    );
}

#[test]
fn devices_asks_every_sot_word() {
    command_asks_each_word("devices", &["devices"], &SOT_WORDS);
}

#[test]
fn load_infrahub_asks_every_sot_word() {
    command_asks_each_word("load-infrahub", &["load-infrahub"], &SOT_WORDS);
}

#[test]
fn maintenance_asks_every_sot_word() {
    command_asks_each_word("maintenance", &["maintenance", "-d", "srl1"], &SOT_WORDS);
}

#[test]
fn schema_load_asks_every_sot_word() {
    command_asks_each_word(
        "schema-load",
        &["schema", "load", "infrahub/schema.yml"],
        &SOT_WORDS,
    );
}

fn spelled(line: &str, name: &str) -> bool {
    line.match_indices(name).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + name.len()..].chars().next();
        let word = |letter: Option<char>| letter.is_some_and(|c| c.is_alphanumeric() || c == '_');
        !word(before) && !word(after)
    })
}

#[test]
fn each_name_is_spelled_in_one_place() {
    let files = product_code();
    assert!(files.len() > 10, "{}", files.len());

    for name in NAMES {
        let places: Vec<String> = files
            .iter()
            .flat_map(|(path, text)| {
                text.lines()
                    .filter(|line| !line.contains("help = \"") && spelled(line, name))
                    .map(move |line| format!("{}: {}", path.display(), line.trim()))
            })
            .collect();
        assert!(
            places.len() == 1 && places[0].contains("status.rs"),
            "{name}: {places:#?}"
        );
    }
    for word in KIND_WORDS {
        let declaration = format!("long = \"{word}\"");
        let count: usize = files
            .iter()
            .map(|(_, text)| text.matches(&declaration).count())
            .sum();
        assert_eq!(count, 1, "{declaration}");
    }
}

fn long_words(command: &clap::Command, path: &str, found: &mut Vec<(String, String, String)>) {
    for (path, command) in commands(command, path) {
        for argument in command.get_arguments() {
            if let Some(long) = argument.get_long() {
                found.push((
                    path.clone(),
                    long.to_string(),
                    argument.get_id().to_string(),
                ));
            }
        }
    }
}

#[test]
fn each_flag_word_of_a_kind_comes_from_its_struct() {
    let sonda_targets = [("flap-interface", "loki-url"), ("incident", "prom-url")];
    let mut found = Vec::new();
    long_words(&Cli::command(), "nobs", &mut found);
    long_words(&WorkshopCli::command(), "nobs <workshop>", &mut found);

    let strays: Vec<&(String, String, String)> = found
        .iter()
        .filter(|(path, long, id)| {
            KIND_WORDS.contains(&long.as_str())
                && id != long
                && !sonda_targets
                    .iter()
                    .any(|(command, word)| path.ends_with(command) && long == word)
        })
        .collect();

    assert!(strays.is_empty(), "{strays:#?}");
    for word in KIND_WORDS {
        assert!(
            found.iter().any(|(_, long, id)| long == word && id == word),
            "{word}"
        );
    }
}
