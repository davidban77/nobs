use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use nobs_core::infrahub::{annotation, read_schema};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::{Seen, Stub};

const TOKEN: &str = "secret-token";
const UNREACHED: &str = "http://192.0.2.1:1";
const REWRITTEN: &str = "INFRAHUB_ADDRESS rewritten to host-reachable http://localhost:8000\n";
const TOKEN_REQUIRED: &str =
    "nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).\n";
const DEFAULT_SINK: &str = "http://loki:3001";

const SCHEMA_LOAD: &str = "/api/schema/load?branch=main";
const SCHEMA: &str = "/api/schema?branch=main";
const GRAPHQL: &str = "/graphql/main";
const APPLIED: &str = r#"{"schema_updated":true}"#;
const KINDS_FOUND: &str = r#"{"nodes":[{"namespace":"Workshop","name":"Device"},{"namespace":"Workshop","name":"Interface"},{"namespace":"Workshop","name":"BgpSession"}]}"#;

const TABLE_CREATED: &str = "Node type           Created  Updated  Total\nWorkshopDevice      2        0        2\nWorkshopInterface   8        0        8\nWorkshopBgpSession  6        0        6";
const TABLE_UPDATED: &str = "Node type           Created  Updated  Total\nWorkshopDevice      0        2        2\nWorkshopInterface   0        8        8\nWorkshopBgpSession  0        6        6";

const SKIPPED: &str = "The next alert for this device will be SKIPPED by the policy.\n";
const EVALUATED: &str = "The next alert for this device will be evaluated normally.\n";

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str, address: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-infrahub-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture(), &root.join("nobs-t-one"));
        let copy = Self(root.canonicalize().unwrap());
        copy.write_env(&format!(
            "INFRAHUB_ADDRESS={address}\nINFRAHUB_API_TOKEN={TOKEN}\n"
        ));
        copy
    }

    fn with_sonda(name: &str, infrahub: &Stub, sonda: &Stub) -> Self {
        let copy = Self::new(name, infrahub.url());
        copy.append_env(&format!("SONDA_SERVER_URL={}\n", sonda.url()));
        copy
    }

    fn dir(&self) -> PathBuf {
        self.0.join("nobs-t-one")
    }

    fn shown(&self) -> String {
        self.dir().display().to_string()
    }

    fn write_env(&self, text: &str) {
        self.write(".env", text);
    }

    fn append_env(&self, text: &str) {
        let path = self.dir().join(".env");
        let mut env = fs::read_to_string(&path).unwrap();
        env.push_str(text);
        fs::write(path, env).unwrap();
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.dir().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
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

fn run(cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command.arg("nobs-t-one").args(args).current_dir(cwd);
    for name in [
        "INFRAHUB_ADDRESS",
        "INFRAHUB_API_TOKEN",
        "SONDA_SERVER_URL",
        "SONDA_API_KEY",
        "SONDA_LOKI_SINK_URL",
        "NOBS_DIR",
    ] {
        command.env_remove(name);
    }
    command.envs(env.iter().copied()).output().unwrap()
}

fn nobs(copy: &Copy, args: &[&str]) -> Output {
    run(&copy.dir(), args, &[])
}

fn nobs_env(copy: &Copy, args: &[&str], env: &[(&str, &str)]) -> Output {
    run(&copy.dir(), args, env)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn filter_answer(kind: &str, id: Option<&str>, maintenance: bool) -> String {
    match id {
        Some(id) => format!(
            r#"{{"data":{{"{kind}":{{"count":1,"edges":[{{"node":{{"id":"{id}","maintenance":{{"value":{maintenance}}}}}}}]}}}}}}"#
        ),
        None => format!(r#"{{"data":{{"{kind}":{{"count":0,"edges":[]}}}}}}"#),
    }
}

fn mutation_answer(operation: &str, id: &str) -> String {
    format!(r#"{{"data":{{"{operation}":{{"ok":true,"object":{{"id":"{id}"}}}}}}}}"#)
}

fn seed_answers(found: bool) -> Vec<String> {
    let mut answers = Vec::new();
    for device in 0..2 {
        for (kind, rows) in [
            ("WorkshopDevice", 1),
            ("WorkshopInterface", 4),
            ("WorkshopBgpSession", 3),
        ] {
            for row in 0..rows {
                let id = format!("{kind}-{device}-{row}");
                if found {
                    answers.push(filter_answer(kind, Some(&id), false));
                    answers.push(mutation_answer(&format!("{kind}Update"), &id));
                } else {
                    answers.push(filter_answer(kind, None, false));
                    answers.push(mutation_answer(&format!("{kind}Create"), &id));
                }
            }
        }
    }
    answers
}

fn maintenance_answers(kind: &str, previous: bool) -> Vec<String> {
    vec![
        filter_answer(kind, Some("node-1"), previous),
        mutation_answer(&format!("{kind}Update"), "node-1"),
    ]
}

fn infrahub(graphql: &[String]) -> Stub {
    let mut routes = vec![
        ("POST", SCHEMA_LOAD, 200, APPLIED),
        ("GET", SCHEMA, 200, KINDS_FOUND),
    ];
    routes.extend(
        graphql
            .iter()
            .map(|answer| ("POST", GRAPHQL, 200, answer.as_str())),
    );
    Stub::start(&routes)
}

fn sonda() -> Stub {
    Stub::start(&[("POST", "/events", 200, "{}")])
}

fn requests(stub: &Stub) -> Vec<String> {
    stub.requests()
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect()
}

fn seed_requests(schema: bool) -> Vec<String> {
    let mut expected = Vec::new();
    if schema {
        expected.push(format!("POST {SCHEMA_LOAD}"));
    }
    expected.push(format!("GET {SCHEMA}"));
    expected.extend((0..32).map(|_| format!("POST {GRAPHQL}")));
    expected
}

fn text(request: &Seen) -> String {
    String::from_utf8_lossy(&request.body).into_owned()
}

fn queries(stub: &Stub) -> Vec<String> {
    stub.requests()
        .iter()
        .filter(|request| request.path == GRAPHQL)
        .map(|request| {
            read_schema(&text(request)).unwrap()["query"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

fn every_request_carries(stub: &Stub, token: &str) {
    let seen = stub.requests();
    assert!(!seen.is_empty());
    for request in &seen {
        assert_eq!(
            request.header("X-INFRAHUB-KEY"),
            Some(token),
            "{} {}",
            request.method,
            request.path
        );
    }
}

fn only_event(stub: &Stub) -> Seen {
    let seen = stub.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].path, "/events");
    assert_eq!(seen[0].header("content-type"), Some("application/json"));
    seen[0].clone()
}

#[test]
fn load_infrahub_applies_the_schema_then_seeds_and_prints_each_step() {
    let stub = infrahub(&seed_answers(false));
    let copy = Copy::new("load", stub.url());
    let (dir, url) = (copy.shown(), stub.url());

    let output = nobs(&copy, &["load-infrahub"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        format!(
            "Applying schema {dir}/infrahub/schema.yml\nSchema applied\nLoading {dir}/lab_vars.yml into Infrahub at {url}\ndevice srl1 created\ndevice srl2 created\n{TABLE_CREATED}\nDone: 16 nodes across 3 types. Open Infrahub at {url}.\n"
        )
    );
    assert_eq!(requests(&stub), seed_requests(true));
    every_request_carries(&stub, TOKEN);
    let sent = read_schema(&text(&stub.requests()[0])).unwrap();
    let file = fs::read_to_string(copy.dir().join("infrahub/schema.yml")).unwrap();
    assert_eq!(sent["schemas"].as_array().unwrap().len(), 1);
    assert_eq!(sent["schemas"][0], read_schema(&file).unwrap());
}

#[test]
fn a_second_load_shows_the_nodes_under_updated_and_warns_of_nothing() {
    let stub = infrahub(&seed_answers(true));
    let copy = Copy::new("reload", stub.url());

    let output = nobs(&copy, &["load-infrahub"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let out = stdout(&output);
    assert!(
        out.contains("device srl1 updated\ndevice srl2 updated\n"),
        "{out}"
    );
    assert!(out.contains(TABLE_UPDATED), "{out}");
    assert!(out.contains("Done: 16 nodes across 3 types."), "{out}");
}

#[test]
fn skip_schema_neither_reads_nor_posts_the_schema_file() {
    let stub = infrahub(&seed_answers(false));
    let copy = Copy::new("skip", stub.url());
    fs::remove_file(copy.dir().join("infrahub/schema.yml")).unwrap();

    let output = nobs(&copy, &["load-infrahub", "--skip-schema"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(requests(&stub), seed_requests(false));
    let out = stdout(&output);
    assert!(!out.contains("Applying schema"), "{out}");
    assert!(!out.contains("Schema applied"), "{out}");
    assert!(out.starts_with("Loading "), "{out}");
}

#[test]
fn a_relative_schema_and_lab_vars_path_is_taken_from_the_workshop_folder() {
    let stub = infrahub(&[
        filter_answer("WorkshopDevice", None, false),
        mutation_answer("WorkshopDeviceCreate", "dev-1"),
    ]);
    let copy = Copy::new("relative", stub.url());
    copy.write("other_schema.yml", "version: \"1.0\"\n");
    copy.write("other_vars.yml", "nodes:\n  leaf1:\n    asn: 65000\n");
    let dir = copy.shown();

    let output = run(
        &copy.dir().join("sub"),
        &[
            "load-infrahub",
            "--schema",
            "other_schema.yml",
            "--lab-vars",
            "other_vars.yml",
        ],
        &[],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.starts_with(&format!(
            "Applying schema {dir}/other_schema.yml\nSchema applied\nLoading {dir}/other_vars.yml into Infrahub at "
        )),
        "{out}"
    );
    assert!(out.contains("device leaf1 created\n"), "{out}");
    assert_eq!(
        text(&stub.requests()[0]),
        r#"{"schemas":[{"version":"1.0"}]}"#
    );
}

#[test]
fn an_absolute_schema_path_stays_as_it_is() {
    let stub = infrahub(&seed_answers(false));
    let copy = Copy::new("absolute", stub.url());
    copy.write("other_schema.yml", "version: \"1.0\"\n");
    let dir = copy.shown();
    let schema = format!("{dir}/other_schema.yml");

    let output = run(
        &copy.dir().join("sub"),
        &["load-infrahub", "--schema", &schema],
        &[],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).starts_with(&format!("Applying schema {schema}\n")),
        "{}",
        stdout(&output)
    );
}

#[test]
fn the_schema_defaults_to_the_schema_file_setting_of_the_workshop() {
    let stub = infrahub(&seed_answers(false));
    let copy = Copy::new("setting", stub.url());
    let toml = fs::read_to_string(copy.dir().join("nobs.toml")).unwrap();
    copy.write(
        "nobs.toml",
        &toml.replace("infrahub/schema.yml", "custom/schema.yml"),
    );
    copy.write("custom/schema.yml", "version: \"1.0\"\n");
    let dir = copy.shown();

    let output = nobs(&copy, &["load-infrahub"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).starts_with(&format!("Applying schema {dir}/custom/schema.yml\n")),
        "{}",
        stdout(&output)
    );
}

#[test]
fn without_a_token_load_infrahub_and_maintenance_say_so_before_they_read_a_file_or_send_anything() {
    let stub = infrahub(&[]);
    let copy = Copy::new("no-token", stub.url());
    copy.write_env(&format!("INFRAHUB_ADDRESS={}\n", stub.url()));
    let commands: [&[&str]; 2] = [
        &["load-infrahub", "--schema", "missing.yml"],
        &["maintenance", "-d", "srl1"],
    ];

    for args in commands {
        let output = nobs(&copy, args);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(stderr(&output), TOKEN_REQUIRED, "{args:?}");
        assert_eq!(stdout(&output), "", "{args:?}");
    }
    assert_eq!(requests(&stub), Vec::<String>::new());
}

#[test]
fn schema_load_checks_the_file_before_the_token() {
    let stub = infrahub(&[]);
    let copy = Copy::new("file-first", stub.url());
    copy.write_env(&format!("INFRAHUB_ADDRESS={}\n", stub.url()));

    let output = nobs(&copy, &["schema", "load", "missing.yml"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: schema file not found: {}/missing.yml\n",
            copy.shown()
        )
    );
    assert_eq!(stdout(&output), "");
    assert_eq!(requests(&stub), Vec::<String>::new());
}

#[test]
fn a_blank_token_in_the_dot_env_file_is_no_token() {
    let stub = infrahub(&[]);
    let copy = Copy::new("blank-env-token", stub.url());
    copy.write_env(&format!(
        "INFRAHUB_ADDRESS={}\nINFRAHUB_API_TOKEN=   \n",
        stub.url()
    ));

    let output = nobs(&copy, &["schema", "load", "infrahub/schema.yml"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), TOKEN_REQUIRED);
    assert_eq!(requests(&stub), Vec::<String>::new());
}

#[test]
fn a_missing_schema_file_exits_1_and_sends_nothing() {
    let stub = infrahub(&[]);
    let copy = Copy::new("missing-schema", stub.url());
    let commands: [&[&str]; 2] = [
        &["load-infrahub", "--schema", "missing.yml"],
        &["schema", "load", "missing.yml"],
    ];

    for args in commands {
        let output = nobs(&copy, args);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            stderr(&output),
            format!(
                "nobs: schema file not found: {}/missing.yml\n",
                copy.shown()
            ),
            "{args:?}"
        );
        assert_eq!(stdout(&output), "", "{args:?}");
    }
    assert_eq!(requests(&stub), Vec::<String>::new());
}

#[test]
fn a_missing_lab_vars_file_exits_1_and_sends_nothing_not_even_the_schema() {
    let stub = infrahub(&[]);
    let copy = Copy::new("missing-vars", stub.url());

    let output = nobs(&copy, &["load-infrahub", "--lab-vars", "missing.yml"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: lab_vars file not found: {}/missing.yml\n",
            copy.shown()
        )
    );
    assert_eq!(stdout(&output), "");
    assert_eq!(requests(&stub), Vec::<String>::new());
}

#[test]
fn a_lab_vars_file_that_does_not_parse_exits_1_and_sends_nothing_not_even_the_schema() {
    let stub = infrahub(&[]);
    let copy = Copy::new("bad-vars", stub.url());
    copy.write("bad_vars.yml", "nodes: [\n");

    let output = nobs(&copy, &["load-infrahub", "--lab-vars", "bad_vars.yml"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with(&format!(
            "nobs: cannot read {}/bad_vars.yml: ",
            copy.shown()
        )),
        "{}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "");
    assert_eq!(requests(&stub), Vec::<String>::new());
}

#[test]
fn a_schema_file_that_does_not_parse_exits_1_and_sends_nothing() {
    let stub = infrahub(&[]);
    let copy = Copy::new("bad-schema", stub.url());
    copy.write("bad_schema.yml", "nodes: [\n");
    let commands: [&[&str]; 2] = [
        &["load-infrahub", "--schema", "bad_schema.yml"],
        &["schema", "load", "bad_schema.yml"],
    ];

    for args in commands {
        let output = nobs(&copy, args);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&output).starts_with(&format!(
                "nobs: cannot read {}/bad_schema.yml: ",
                copy.shown()
            )),
            "{args:?}: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output), "", "{args:?}");
    }
    assert_eq!(requests(&stub), Vec::<String>::new());
}

#[test]
fn a_lab_vars_file_with_no_nodes_is_a_warning_on_standard_error_and_exit_0() {
    let stub = infrahub(&[]);
    let copy = Copy::new("no-devices", stub.url());
    copy.write("empty_vars.yml", "prefixes: []\n");

    let output = nobs(
        &copy,
        &[
            "load-infrahub",
            "--skip-schema",
            "--lab-vars",
            "empty_vars.yml",
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: no devices found in {}/empty_vars.yml: is --lab-vars the right file?\n",
            copy.shown()
        )
    );
    assert!(
        stdout(&output).contains("Done: 0 nodes across 3 types."),
        "{}",
        stdout(&output)
    );
}

#[test]
fn schema_load_applies_the_file_and_prints_the_two_lines() {
    let stub = infrahub(&[]);
    let copy = Copy::new("schema-load", stub.url());
    let dir = copy.shown();

    let output = nobs(&copy, &["schema", "load", "infrahub/schema.yml"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        format!("Applying schema {dir}/infrahub/schema.yml\nSchema applied\n")
    );
    assert_eq!(requests(&stub), [format!("POST {SCHEMA_LOAD}")]);
    every_request_carries(&stub, TOKEN);
}

#[test]
fn an_answer_that_is_not_200_to_the_schema_exits_1_without_saying_it_was_applied() {
    let stub = Stub::start(&[("POST", SCHEMA_LOAD, 401, "bad token")]);
    let copy = Copy::new("schema-401", stub.url());
    let commands: [&[&str]; 2] = [
        &["load-infrahub"],
        &["schema", "load", "infrahub/schema.yml"],
    ];

    for args in commands {
        let output = nobs(&copy, args);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            stderr(&output),
            "nobs: Infrahub answered 401 on /api/schema/load?branch=main: bad token\n",
            "{args:?}"
        );
        assert_eq!(
            stdout(&output),
            format!("Applying schema {}/infrahub/schema.yml\n", copy.shown()),
            "{args:?}"
        );
    }
    assert_eq!(requests(&stub), vec![format!("POST {SCHEMA_LOAD}"); 2]);
}

#[test]
fn an_answer_that_is_not_200_to_the_kind_wait_exits_1_before_the_seed() {
    let stub = Stub::start(&[("GET", SCHEMA, 401, "no")]);
    let copy = Copy::new("wait-401", stub.url());

    let output = nobs(&copy, &["load-infrahub", "--skip-schema"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: Infrahub answered 401 on /api/schema?branch=main: no\n"
    );
    assert_eq!(
        stdout(&output),
        format!(
            "Loading {}/lab_vars.yml into Infrahub at {}\n",
            copy.shown(),
            stub.url()
        )
    );
    assert_eq!(requests(&stub), [format!("GET {SCHEMA}")]);
}

#[test]
fn a_graphql_error_exits_1_with_its_message_and_prints_no_table() {
    let stub = infrahub(&[r#"{"errors":[{"message":"boom"}]}"#.to_string()]);
    let copy = Copy::new("graphql-error", stub.url());

    let output = nobs(&copy, &["load-infrahub", "--skip-schema"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "nobs: Infrahub: boom\n");
    let out = stdout(&output);
    for later in ["device ", "Node type", "Done:"] {
        assert!(!out.contains(later), "{out}");
    }
}

#[test]
fn an_infrahub_that_cannot_be_reached_exits_1_with_the_address_and_the_reason() {
    let closed = closed_port_url();
    let copy = Copy::new("unreachable", &closed);

    let output = nobs(&copy, &["schema", "load", "infrahub/schema.yml"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with(&format!("nobs: cannot reach {closed}: ")),
        "{}",
        stderr(&output)
    );
    assert!(!stdout(&output).contains("Schema applied"));
}

#[test]
fn the_address_and_token_flags_beat_the_dot_env_file_and_a_trailing_slash_is_dropped() {
    let stub = infrahub(&[]);
    let copy = Copy::new("flags", UNREACHED);

    let output = nobs(
        &copy,
        &[
            "schema",
            "load",
            "infrahub/schema.yml",
            "--address",
            &format!("{}/", stub.url()),
            "--token",
            "flag-token",
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(requests(&stub), [format!("POST {SCHEMA_LOAD}")]);
    every_request_carries(&stub, "flag-token");
    assert!(
        !stdout(&output).contains("rewritten"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_blank_flag_counts_as_not_given() {
    let stub = infrahub(&[]);
    let copy = Copy::new("blank-flags", stub.url());

    let output = nobs(
        &copy,
        &[
            "schema",
            "load",
            "infrahub/schema.yml",
            "--address",
            " ",
            "--token",
            " ",
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(requests(&stub), [format!("POST {SCHEMA_LOAD}")]);
    every_request_carries(&stub, TOKEN);
}

#[test]
fn an_address_that_names_the_container_is_rewritten_to_localhost_and_the_line_comes_first() {
    let copy = Copy::new("rewrite", "http://infrahub-server:8000");
    copy.write("bad_schema.yml", "nodes: [\n");
    let commands: [(&[&str], &str); 2] = [
        (&["schema", "load", "bad_schema.yml"], "nobs: cannot read "),
        (
            &["load-infrahub", "--lab-vars", "missing.yml"],
            "nobs: lab_vars file not found: ",
        ),
    ];

    for (args, error) in commands {
        let output = nobs(&copy, args);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(stdout(&output), REWRITTEN, "{args:?}");
        assert!(stderr(&output).starts_with(error), "{args:?}");
    }
}

#[test]
fn the_rewrite_line_comes_before_the_token_and_file_checks_of_all_three_commands() {
    let copy = Copy::new("rewrite-first", "http://infrahub-server:8000");
    copy.write_env("INFRAHUB_ADDRESS=http://infrahub-server:8000\n");
    let missing = format!(
        "nobs: schema file not found: {}/missing.yml\n",
        copy.shown()
    );
    let commands: [(&[&str], &str); 4] = [
        (&["load-infrahub"], TOKEN_REQUIRED),
        (&["schema", "load", "infrahub/schema.yml"], TOKEN_REQUIRED),
        (&["maintenance", "-d", "srl1"], TOKEN_REQUIRED),
        (&["schema", "load", "missing.yml"], &missing),
    ];

    for (args, error) in commands {
        let output = nobs(&copy, args);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(stdout(&output), REWRITTEN, "{args:?}");
        assert_eq!(stderr(&output), error, "{args:?}");
    }
}

#[test]
fn a_blank_infrahub_token_in_the_shell_lets_the_dot_env_token_through() {
    let stub = infrahub(&[]);
    let copy = Copy::new("blank-shell-token", stub.url());

    let output = nobs_env(
        &copy,
        &["schema", "load", "infrahub/schema.yml"],
        &[("INFRAHUB_API_TOKEN", "  ")],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    every_request_carries(&stub, TOKEN);
}

#[test]
fn a_blank_sonda_server_url_in_the_shell_lets_the_dot_env_address_through() {
    let stub = infrahub(&maintenance_answers("WorkshopDevice", false));
    let events = sonda();
    let copy = Copy::with_sonda("blank-shell-sonda", &stub, &events);

    let output = nobs_env(
        &copy,
        &["maintenance", "-d", "srl1"],
        &[("SONDA_SERVER_URL", "  ")],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    only_event(&events);
}

#[test]
fn a_blank_sonda_key_in_the_shell_lets_the_dot_env_key_through() {
    let stub = infrahub(&maintenance_answers("WorkshopDevice", false));
    let events = sonda();
    let copy = Copy::with_sonda("blank-shell-key", &stub, &events);
    copy.append_env("SONDA_API_KEY=sonda-key\n");

    let output = nobs_env(
        &copy,
        &["maintenance", "-d", "srl1"],
        &[("SONDA_API_KEY", "  ")],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        only_event(&events).header("authorization"),
        Some("Bearer sonda-key")
    );
}

#[test]
fn a_blank_sonda_sink_in_the_shell_lets_the_dot_env_sink_through() {
    let stub = infrahub(&maintenance_answers("WorkshopDevice", false));
    let events = sonda();
    let copy = Copy::with_sonda("blank-shell-sink", &stub, &events);
    copy.append_env("SONDA_LOKI_SINK_URL=http://sink:3100\n");

    let output = nobs_env(
        &copy,
        &["maintenance", "-d", "srl1"],
        &[("SONDA_LOKI_SINK_URL", "  ")],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        only_event(&events).body,
        annotation("srl1", true, "http://sink:3100").as_bytes()
    );
}

#[test]
fn a_blank_sonda_key_or_sink_in_the_dot_env_file_counts_as_not_given() {
    let stub = infrahub(&maintenance_answers("WorkshopDevice", false));
    let events = sonda();
    let copy = Copy::with_sonda("blank-env-sonda", &stub, &events);
    copy.append_env("SONDA_API_KEY=   \nSONDA_LOKI_SINK_URL=   \n");

    let output = nobs(&copy, &["maintenance", "-d", "srl1"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let event = only_event(&events);
    assert_eq!(event.header("authorization"), None);
    assert_eq!(
        event.body,
        annotation("srl1", true, DEFAULT_SINK).as_bytes()
    );
}

#[test]
fn maintenance_sets_the_flag_prints_the_change_and_posts_the_annotation_to_sonda() {
    let stub = infrahub(&maintenance_answers("WorkshopDevice", false));
    let events = sonda();
    let copy = Copy::with_sonda("maintenance-state", &stub, &events);

    let output = nobs(&copy, &["maintenance", "-d", "srl1", "--state"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        format!("srl1.maintenance: false → true\n{SKIPPED}")
    );
    let sent = queries(&stub);
    assert_eq!(sent.len(), 2);
    assert!(
        sent[0].starts_with("query { WorkshopDevice("),
        "{}",
        sent[0]
    );
    assert!(sent[0].contains(r#"name__value: "srl1""#), "{}", sent[0]);
    assert!(
        sent[1].starts_with("mutation { WorkshopDeviceUpdate("),
        "{}",
        sent[1]
    );
    assert!(
        sent[1].contains("maintenance: {value: true}"),
        "{}",
        sent[1]
    );
    every_request_carries(&stub, TOKEN);
    assert_eq!(
        only_event(&events).body,
        annotation("srl1", true, DEFAULT_SINK).as_bytes()
    );
}

#[test]
fn maintenance_clear_sets_false_and_says_the_next_alert_is_evaluated_normally() {
    let stub = infrahub(&maintenance_answers("WorkshopDevice", true));
    let events = sonda();
    let copy = Copy::with_sonda("maintenance-clear", &stub, &events);

    let output = nobs(&copy, &["maintenance", "--device", "srl1", "--clear"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!("srl1.maintenance: true → false\n{EVALUATED}")
    );
    assert!(
        queries(&stub)[1].contains("maintenance: {value: false}"),
        "{}",
        queries(&stub)[1]
    );
    assert_eq!(
        only_event(&events).body,
        annotation("srl1", false, DEFAULT_SINK).as_bytes()
    );
}

#[test]
fn maintenance_sets_the_flag_by_default_and_the_last_of_state_and_clear_wins() {
    let answers = [
        maintenance_answers("WorkshopDevice", false),
        maintenance_answers("WorkshopDevice", false),
        maintenance_answers("WorkshopDevice", false),
    ]
    .concat();
    let stub = infrahub(&answers);
    let events = sonda();
    let copy = Copy::with_sonda("maintenance-last", &stub, &events);
    let runs: [(&[&str], bool); 3] = [
        (&["maintenance", "-d", "srl1"], true),
        (&["maintenance", "-d", "srl1", "--clear", "--state"], true),
        (&["maintenance", "-d", "srl1", "--state", "--clear"], false),
    ];

    for (args, _) in runs {
        let output = nobs(&copy, args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stderr(&output)
        );
    }

    let sent = queries(&stub);
    for (run, (args, state)) in runs.iter().enumerate() {
        assert!(
            sent[run * 2 + 1].contains(&format!("maintenance: {{value: {state}}}")),
            "{args:?}: {}",
            sent[run * 2 + 1]
        );
    }
}

#[test]
fn maintenance_kind_is_the_root_field_of_the_requests() {
    let stub = infrahub(&maintenance_answers("Other", false));
    let events = sonda();
    let copy = Copy::with_sonda("maintenance-kind", &stub, &events);

    let output = nobs(&copy, &["maintenance", "-d", "srl1", "--kind", "Other"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let sent = queries(&stub);
    assert!(sent[0].starts_with("query { Other("), "{}", sent[0]);
    assert!(
        sent[1].starts_with("mutation { OtherUpdate("),
        "{}",
        sent[1]
    );
}

#[test]
fn a_device_infrahub_does_not_have_exits_1_and_posts_nothing_to_sonda() {
    let stub = infrahub(&[filter_answer("WorkshopDevice", None, false)]);
    let events = sonda();
    let copy = Copy::with_sonda("maintenance-not-found", &stub, &events);

    let output = nobs(&copy, &["maintenance", "-d", "srl1"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: WorkshopDevice srl1 not found in Infrahub.\n"
    );
    assert_eq!(stdout(&output), "");
    assert_eq!(queries(&stub).len(), 1);
    assert_eq!(events.requests().len(), 0);
}

#[test]
fn an_infrahub_that_refuses_the_flag_exits_1_and_posts_nothing_to_sonda() {
    let stub = Stub::start(&[("POST", GRAPHQL, 401, "bad token")]);
    let events = sonda();
    let copy = Copy::with_sonda("maintenance-401", &stub, &events);

    let output = nobs(&copy, &["maintenance", "-d", "srl1"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: Infrahub answered 401 on /graphql/main: bad token\n"
    );
    assert_eq!(stdout(&output), "");
    assert_eq!(events.requests().len(), 0);
}

#[test]
fn a_sonda_that_answers_500_exits_1_after_the_flag_was_sent() {
    let stub = infrahub(&maintenance_answers("WorkshopDevice", false));
    let events = Stub::start(&[("POST", "/events", 500, "boom")]);
    let copy = Copy::with_sonda("maintenance-500", &stub, &events);

    let output = nobs(&copy, &["maintenance", "-d", "srl1"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: sonda /events post failed: sonda answered 500: boom\n"
    );
    assert_eq!(stdout(&output), "srl1.maintenance: false → true\n");
    assert_eq!(queries(&stub).len(), 2);
}

#[test]
fn a_sonda_that_cannot_be_reached_exits_1_with_the_clients_text() {
    let stub = infrahub(&maintenance_answers("WorkshopDevice", false));
    let copy = Copy::new("maintenance-closed", stub.url());
    let closed = closed_port_url();
    copy.append_env(&format!("SONDA_SERVER_URL={closed}\n"));

    let output = nobs(&copy, &["maintenance", "-d", "srl1"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with(&format!(
            "nobs: sonda /events post failed: cannot reach {closed}: "
        )),
        "{}",
        stderr(&output)
    );
    assert!(!stdout(&output).contains("The next alert"));
}
