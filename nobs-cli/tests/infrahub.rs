use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use nobs_core::infrahub::annotation;

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

const ENV: &str = "INFRAHUB_ADDRESS=http://infrahub-server:8000\nINFRAHUB_API_TOKEN=secret-token\n";
const TOKEN: &str = "secret-token";
const HOST_ADDRESS: &str = "http://localhost:8000";
const REWRITTEN: &str = "INFRAHUB_ADDRESS rewritten to host-reachable http://localhost:8000\n";
const TOKEN_REQUIRED: &str =
    "nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).\n";
const SCRIPT_EXITED: &str = "nobs: the Infrahub script exited 1\n";
const DEFAULT_SINK: &str = "http://loki:3001";

const SEED: &str = r#"{"WorkshopDevice": {"created": 1, "updated": 2}, "WorkshopInterface": {"created": 3, "updated": 4}, "WorkshopBgpSession": {"created": 5, "updated": 6}}"#;
const NO_DEVICES: &str = r#"{"WorkshopDevice": {"created": 0, "updated": 0}, "WorkshopInterface": {"created": 3, "updated": 4}, "WorkshopBgpSession": {"created": 5, "updated": 6}}"#;
const UPDATED_DEVICES: &str = r#"{"WorkshopDevice": {"created": 0, "updated": 2}, "WorkshopInterface": {"created": 0, "updated": 4}, "WorkshopBgpSession": {"created": 0, "updated": 6}}"#;
const FOUND_FALSE_TO_TRUE: &str = r#"{"found": true, "previous": false, "state": true}"#;
const FOUND_TRUE_TO_FALSE: &str = r#"{"found": true, "previous": true, "state": false}"#;
const NOT_FOUND: &str = r#"{"found": false}"#;

const TABLE: &str = "Node type           Created  Updated  Total\nWorkshopDevice      1        2        3\nWorkshopInterface   3        4        7\nWorkshopBgpSession  5        6        11";

const SKIPPED: &str = "The next alert for this device will be SKIPPED by the policy.\n";
const EVALUATED: &str = "The next alert for this device will be evaluated normally.\n";

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-infrahub-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture(), &root.join("nobs-t-one"));
        let copy = Self(root.canonicalize().unwrap());
        fs::create_dir_all(copy.bin()).unwrap();
        fs::create_dir_all(copy.empty()).unwrap();
        let uv = copy.bin().join("uv");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/support/fake-uv.sh"),
            &uv,
        )
        .unwrap();
        fs::set_permissions(&uv, fs::Permissions::from_mode(0o755)).unwrap();
        copy.write_env(ENV);
        copy
    }

    fn dir(&self) -> PathBuf {
        self.0.join("nobs-t-one")
    }

    fn bin(&self) -> PathBuf {
        self.0.join("bin")
    }

    fn empty(&self) -> PathBuf {
        self.0.join("empty")
    }

    fn log(&self) -> PathBuf {
        self.0.join("uv.log")
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

    fn shown(&self) -> String {
        self.dir().display().to_string()
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

fn run(copy: &Copy, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let path = format!(
        "{}:{}",
        copy.bin().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let reply = if args.first() == Some(&"maintenance") {
        FOUND_FALSE_TO_TRUE
    } else {
        SEED
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_nobs"));
    command.arg("nobs-t-one").args(args).current_dir(cwd);
    for name in [
        "INFRAHUB_ADDRESS",
        "INFRAHUB_API_TOKEN",
        "SONDA_SERVER_URL",
        "SONDA_API_KEY",
        "SONDA_LOKI_SINK_URL",
        "NOBS_DIR",
        "FAKE_UV_CODE",
    ] {
        command.env_remove(name);
    }
    command
        .env("PATH", path)
        .env("FAKE_UV_LOG", copy.log())
        .env("FAKE_UV_STDOUT", reply)
        .envs(env.iter().copied());
    command.output().unwrap()
}

fn nobs(copy: &Copy, args: &[&str]) -> Output {
    run(copy, &copy.dir(), args, &[])
}

fn nobs_env(copy: &Copy, args: &[&str], env: &[(&str, &str)]) -> Output {
    run(copy, &copy.dir(), args, env)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

struct Logged {
    args: String,
    address: String,
    token: String,
}

impl Logged {
    fn words(&self) -> &str {
        let rest = self
            .args
            .strip_prefix("run --quiet --script ")
            .expect("uv was run with run --quiet --script");
        rest.split_once(' ').map_or("", |(_, words)| words)
    }
}

fn logged(copy: &Copy) -> Vec<Logged> {
    fs::read_to_string(copy.log())
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let mut parts = line.split('\t').map(str::to_string);
            Logged {
                args: parts.next().unwrap(),
                address: parts.next().unwrap(),
                token: parts.next().unwrap(),
            }
        })
        .collect()
}

fn calls(copy: &Copy) -> Vec<String> {
    logged(copy)
        .iter()
        .map(|call| call.words().to_string())
        .collect()
}

fn no_call(copy: &Copy) {
    assert_eq!(calls(copy), Vec::<String>::new());
}

fn event_stub() -> Stub {
    Stub::start(&[("POST", "/events", 200, "{}")])
}

fn sonda_copy(name: &str, stub: &Stub) -> Copy {
    let copy = Copy::new(name);
    copy.append_env(&format!("SONDA_SERVER_URL={}\n", stub.url()));
    copy
}

#[test]
fn load_infrahub_applies_the_schema_then_loads_the_lab_vars_and_prints_each_step() {
    let copy = Copy::new("load");
    let dir = copy.shown();

    let output = nobs(&copy, &["load-infrahub"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        format!(
            "{REWRITTEN}Applying schema {dir}/infrahub/schema.yml\nSchema applied\nLoading {dir}/lab_vars.yml into Infrahub at {HOST_ADDRESS}\n{TABLE}\nDone: 21 nodes across 3 types. Open Infrahub at {HOST_ADDRESS}.\n"
        )
    );
    assert_eq!(
        calls(&copy),
        [
            format!("schema {dir}/infrahub/schema.yml"),
            format!("seed {dir}/lab_vars.yml")
        ]
    );
    for call in logged(&copy) {
        assert_eq!(call.address, HOST_ADDRESS);
        assert_eq!(call.token, TOKEN);
        assert!(!call.args.contains(TOKEN), "{}", call.args);
    }
}

#[test]
fn skip_schema_does_not_apply_the_schema() {
    let copy = Copy::new("skip");
    let dir = copy.shown();

    let output = nobs(&copy, &["load-infrahub", "--skip-schema"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(calls(&copy), [format!("seed {dir}/lab_vars.yml")]);
    let out = stdout(&output);
    assert!(!out.contains("Applying schema"), "{out}");
    assert!(!out.contains("Schema applied"), "{out}");
    assert!(out.contains("Loading "), "{out}");
}

#[test]
fn skip_schema_does_not_need_the_schema_file() {
    let copy = Copy::new("skip-missing");
    fs::remove_file(copy.dir().join("infrahub/schema.yml")).unwrap();

    let output = nobs(&copy, &["load-infrahub", "--skip-schema"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(calls(&copy).len(), 1);
}

#[test]
fn a_relative_schema_and_lab_vars_path_is_taken_from_the_workshop_folder() {
    let copy = Copy::new("relative");
    copy.write("other_schema.yml", "version: \"1.0\"\n");
    copy.write("other_vars.yml", "devices: []\n");
    let dir = copy.shown();

    let output = run(
        &copy,
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
    assert_eq!(
        calls(&copy),
        [
            format!("schema {dir}/other_schema.yml"),
            format!("seed {dir}/other_vars.yml")
        ]
    );
    let out = stdout(&output);
    assert!(out.contains(&format!("Applying schema {dir}/other_schema.yml\n")));
    assert!(out.contains(&format!("Loading {dir}/other_vars.yml into")));
}

#[test]
fn an_absolute_schema_and_lab_vars_path_stays_as_it_is() {
    let copy = Copy::new("absolute");
    copy.write("other_schema.yml", "version: \"1.0\"\n");
    copy.write("other_vars.yml", "devices: []\n");
    let dir = copy.shown();
    let schema = format!("{dir}/other_schema.yml");
    let lab_vars = format!("{dir}/other_vars.yml");

    let output = run(
        &copy,
        &copy.dir().join("sub"),
        &[
            "load-infrahub",
            "--schema",
            &schema,
            "--lab-vars",
            &lab_vars,
        ],
        &[],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        calls(&copy),
        [format!("schema {schema}"), format!("seed {lab_vars}")]
    );
}

#[test]
fn the_schema_defaults_to_the_schema_file_setting_of_the_workshop() {
    let copy = Copy::new("setting");
    let toml = fs::read_to_string(copy.dir().join("nobs.toml")).unwrap();
    copy.write(
        "nobs.toml",
        &toml.replace("infrahub/schema.yml", "custom/schema.yml"),
    );
    copy.write("custom/schema.yml", "version: \"1.0\"\n");
    let dir = copy.shown();

    let output = nobs(&copy, &["load-infrahub"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(calls(&copy)[0], format!("schema {dir}/custom/schema.yml"));
}

#[test]
fn without_a_token_each_command_says_so_exits_1_and_runs_nothing() {
    let copy = Copy::new("no-token");
    copy.write_env("INFRAHUB_ADDRESS=http://infrahub-server:8000\n");
    let commands: [&[&str]; 3] = [
        &["load-infrahub"],
        &["schema", "load", "infrahub/schema.yml"],
        &["maintenance", "-d", "srl1"],
    ];

    for args in commands {
        let output = nobs(&copy, args);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(stderr(&output), TOKEN_REQUIRED, "{args:?}");
        assert_eq!(stdout(&output), "", "{args:?}");
    }
    no_call(&copy);
}

#[test]
fn a_blank_token_in_the_dot_env_file_is_no_token() {
    let copy = Copy::new("blank-env-token");
    copy.write_env("INFRAHUB_ADDRESS=http://infrahub-server:8000\nINFRAHUB_API_TOKEN=   \n");

    let output = nobs(&copy, &["load-infrahub"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), TOKEN_REQUIRED);
    no_call(&copy);
}

#[test]
fn load_infrahub_checks_the_token_before_the_files() {
    let copy = Copy::new("token-first");
    copy.write_env("INFRAHUB_ADDRESS=http://infrahub-server:8000\n");

    let output = nobs(&copy, &["load-infrahub", "--schema", "missing.yml"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), TOKEN_REQUIRED);
}

#[test]
fn schema_load_checks_the_file_before_the_token() {
    let copy = Copy::new("file-first");
    copy.write_env("INFRAHUB_ADDRESS=http://infrahub-server:8000\n");

    let output = nobs(&copy, &["schema", "load", "missing.yml"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: schema file not found: {}/missing.yml\n",
            copy.shown()
        )
    );
}

#[test]
fn a_missing_schema_file_exits_1_and_runs_nothing() {
    let copy = Copy::new("missing-schema");

    let output = nobs(&copy, &["load-infrahub", "--schema", "missing.yml"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: schema file not found: {}/missing.yml\n",
            copy.shown()
        )
    );
    assert_eq!(stdout(&output), "");
    no_call(&copy);
}

#[test]
fn a_missing_lab_vars_file_exits_1_and_runs_nothing_not_even_the_schema() {
    let copy = Copy::new("missing-vars");

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
    no_call(&copy);
}

#[test]
fn schema_load_of_a_missing_file_exits_1_and_runs_nothing() {
    let copy = Copy::new("schema-missing");

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
    no_call(&copy);
}

#[test]
fn zero_devices_created_and_updated_is_a_warning_on_standard_error_and_exit_0() {
    let copy = Copy::new("no-devices");

    let output = nobs_env(&copy, &["load-infrahub"], &[("FAKE_UV_STDOUT", NO_DEVICES)]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stderr(&output),
        format!(
            "nobs: no devices found in {}/lab_vars.yml: is --lab-vars the right file?\n",
            copy.shown()
        )
    );
    assert!(
        stdout(&output).contains("Done: 18 nodes across 3 types."),
        "{}",
        stdout(&output)
    );
}

#[test]
fn updated_devices_alone_are_not_a_warning() {
    let copy = Copy::new("updated-devices");

    let output = nobs_env(
        &copy,
        &["load-infrahub"],
        &[("FAKE_UV_STDOUT", UPDATED_DEVICES)],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert!(
        stdout(&output).contains("Done: 12 nodes across 3 types."),
        "{}",
        stdout(&output)
    );
}

#[test]
fn schema_load_applies_the_file_and_prints_the_two_lines() {
    let copy = Copy::new("schema-load");
    let dir = copy.shown();

    let output = nobs(&copy, &["schema", "load", "infrahub/schema.yml"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        format!("{REWRITTEN}Applying schema {dir}/infrahub/schema.yml\nSchema applied\n")
    );
    assert_eq!(calls(&copy), [format!("schema {dir}/infrahub/schema.yml")]);
    let call = &logged(&copy)[0];
    assert_eq!(call.address, HOST_ADDRESS);
    assert_eq!(call.token, TOKEN);
    assert!(!call.args.contains(TOKEN), "{}", call.args);
}

#[test]
fn a_schema_that_fails_stops_load_infrahub_before_the_seed() {
    let copy = Copy::new("schema-fails");

    let output = nobs_env(&copy, &["load-infrahub"], &[("FAKE_UV_CODE", "1")]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), SCRIPT_EXITED);
    let out = stdout(&output);
    assert!(out.contains("Applying schema "), "{out}");
    for later in ["Schema applied", "Loading ", "Done:"] {
        assert!(!out.contains(later), "{out}");
    }
    assert_eq!(calls(&copy).len(), 1);
}

#[test]
fn a_seed_that_fails_prints_no_table_and_no_done_line() {
    let copy = Copy::new("seed-fails");

    let output = nobs_env(
        &copy,
        &["load-infrahub", "--skip-schema"],
        &[("FAKE_UV_CODE", "1")],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), SCRIPT_EXITED);
    let out = stdout(&output);
    for later in ["Node type", "Done:"] {
        assert!(!out.contains(later), "{out}");
    }
}

#[test]
fn a_script_that_printed_text_nobs_cannot_read_exits_1() {
    let copy = Copy::new("garbage");

    let output = nobs_env(
        &copy,
        &["load-infrahub", "--skip-schema"],
        &[("FAKE_UV_STDOUT", "Traceback")],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output)
            .starts_with("nobs: the Infrahub script answered with text nobs cannot read: "),
        "{}",
        stderr(&output)
    );
    assert!(!stdout(&output).contains("Done:"));
}

#[test]
fn a_schema_load_that_fails_does_not_say_the_schema_was_applied() {
    let copy = Copy::new("schema-load-fails");

    let output = nobs_env(
        &copy,
        &["schema", "load", "infrahub/schema.yml"],
        &[("FAKE_UV_CODE", "1")],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), SCRIPT_EXITED);
    assert!(!stdout(&output).contains("Schema applied"));
}

#[test]
fn a_maintenance_script_that_fails_exits_1_and_posts_nothing() {
    let stub = event_stub();
    let copy = sonda_copy("maintenance-fails", &stub);

    let output = nobs_env(
        &copy,
        &["maintenance", "-d", "srl1"],
        &[("FAKE_UV_CODE", "1")],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), SCRIPT_EXITED);
    assert!(!stdout(&output).contains(".maintenance:"));
    assert!(stub.requests().is_empty());
}

#[test]
fn without_uv_on_the_path_each_command_says_it_cannot_run_uv_and_exits_1() {
    let copy = Copy::new("no-uv");
    let empty = copy.empty().display().to_string();
    let commands: [&[&str]; 3] = [
        &["load-infrahub"],
        &["schema", "load", "infrahub/schema.yml"],
        &["maintenance", "-d", "srl1"],
    ];

    for args in commands {
        let output = nobs_env(&copy, args, &[("PATH", &empty)]);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&output).starts_with("nobs: cannot run uv: "),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(!stdout(&output).contains("Schema applied"), "{args:?}");
    }
}

#[test]
fn the_address_and_token_flags_beat_the_dot_env_file() {
    let copy = Copy::new("flags");

    let output = nobs(
        &copy,
        &[
            "load-infrahub",
            "--skip-schema",
            "--address",
            "http://10.0.0.5:8000/",
            "--token",
            "flag-token",
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let call = &logged(&copy)[0];
    assert_eq!(call.address, "http://10.0.0.5:8000");
    assert_eq!(call.token, "flag-token");
    assert!(!call.args.contains("flag-token"), "{}", call.args);
    let out = stdout(&output);
    assert!(!out.contains("rewritten"), "{out}");
    assert!(out.contains("Loading "), "{out}");
    assert!(
        out.contains("Open Infrahub at http://10.0.0.5:8000."),
        "{out}"
    );
}

#[test]
fn without_an_address_the_default_is_localhost_8000_and_says_nothing_of_a_rewrite() {
    let copy = Copy::new("default-address");
    copy.write_env("INFRAHUB_API_TOKEN=secret-token\n");

    let output = nobs(&copy, &["load-infrahub", "--skip-schema"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(logged(&copy)[0].address, HOST_ADDRESS);
    assert!(!stdout(&output).contains("rewritten"));
}

#[test]
fn a_blank_flag_counts_as_not_given() {
    let copy = Copy::new("blank-flags");

    let output = nobs(
        &copy,
        &[
            "load-infrahub",
            "--skip-schema",
            "--address",
            " ",
            "--token",
            " ",
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let call = &logged(&copy)[0];
    assert_eq!(call.address, HOST_ADDRESS);
    assert_eq!(call.token, TOKEN);
}

#[test]
fn a_blank_token_in_the_shell_lets_the_dot_env_token_through() {
    let copy = Copy::new("blank-shell-token");

    let output = nobs_env(
        &copy,
        &["load-infrahub", "--skip-schema"],
        &[("INFRAHUB_API_TOKEN", "  ")],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(logged(&copy)[0].token, TOKEN);
}

#[test]
fn maintenance_sets_the_flag_prints_the_change_and_posts_the_annotation_to_sonda() {
    let stub = event_stub();
    let copy = sonda_copy("maintenance-state", &stub);

    let output = nobs_env(
        &copy,
        &["maintenance", "-d", "srl1", "--state"],
        &[("FAKE_UV_STDOUT", FOUND_FALSE_TO_TRUE)],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        format!("{REWRITTEN}srl1.maintenance: false → true\n{SKIPPED}")
    );
    assert_eq!(
        calls(&copy),
        ["maintenance srl1 true --kind WorkshopDevice"]
    );
    let call = &logged(&copy)[0];
    assert_eq!(call.address, HOST_ADDRESS);
    assert_eq!(call.token, TOKEN);
    assert!(!call.args.contains(TOKEN), "{}", call.args);
    let requests = stub.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/events");
    assert_eq!(requests[0].header("content-type"), Some("application/json"));
    assert_eq!(
        requests[0].body,
        annotation("srl1", true, DEFAULT_SINK).as_bytes()
    );
}

#[test]
fn maintenance_clear_sets_false_and_says_the_next_alert_is_evaluated_normally() {
    let stub = event_stub();
    let copy = sonda_copy("maintenance-clear", &stub);

    let output = nobs_env(
        &copy,
        &["maintenance", "--device", "srl1", "--clear"],
        &[("FAKE_UV_STDOUT", FOUND_TRUE_TO_FALSE)],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!("{REWRITTEN}srl1.maintenance: true → false\n{EVALUATED}")
    );
    assert_eq!(
        calls(&copy),
        ["maintenance srl1 false --kind WorkshopDevice"]
    );
    let requests = stub.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body,
        annotation("srl1", false, DEFAULT_SINK).as_bytes()
    );
}

#[test]
fn maintenance_sets_the_flag_by_default_and_the_last_of_state_and_clear_wins() {
    let stub = event_stub();
    let copy = sonda_copy("maintenance-last", &stub);
    let runs: [(&[&str], &str); 3] = [
        (&["maintenance", "-d", "srl1"], "true"),
        (&["maintenance", "-d", "srl1", "--clear", "--state"], "true"),
        (
            &["maintenance", "-d", "srl1", "--state", "--clear"],
            "false",
        ),
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

    let expected: Vec<String> = runs
        .iter()
        .map(|(_, state)| format!("maintenance srl1 {state} --kind WorkshopDevice"))
        .collect();
    assert_eq!(calls(&copy), expected);
}

#[test]
fn maintenance_kind_is_handed_to_the_script() {
    let stub = event_stub();
    let copy = sonda_copy("maintenance-kind", &stub);

    let output = nobs_env(
        &copy,
        &["maintenance", "-d", "srl1", "--kind", "Other"],
        &[("FAKE_UV_STDOUT", FOUND_FALSE_TO_TRUE)],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(calls(&copy), ["maintenance srl1 true --kind Other"]);
}

#[test]
fn a_device_infrahub_does_not_have_exits_1_and_posts_nothing() {
    let stub = event_stub();
    let copy = sonda_copy("maintenance-not-found", &stub);

    let output = nobs_env(
        &copy,
        &["maintenance", "-d", "srl1"],
        &[("FAKE_UV_STDOUT", NOT_FOUND)],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: WorkshopDevice srl1 not found in Infrahub.\n"
    );
    assert_eq!(stdout(&output), REWRITTEN);
    assert!(stub.requests().is_empty());
}

#[test]
fn a_sonda_that_answers_500_exits_1_after_the_flag_was_saved() {
    let stub = Stub::start(&[("POST", "/events", 500, "boom")]);
    let copy = sonda_copy("maintenance-500", &stub);

    let output = nobs_env(
        &copy,
        &["maintenance", "-d", "srl1"],
        &[("FAKE_UV_STDOUT", FOUND_FALSE_TO_TRUE)],
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "nobs: sonda /events post failed: sonda answered 500: boom\n"
    );
    let out = stdout(&output);
    assert!(out.contains("srl1.maintenance: false → true\n"), "{out}");
    assert!(!out.contains("The next alert"), "{out}");
    assert_eq!(calls(&copy).len(), 1);
}

#[test]
fn a_sonda_that_cannot_be_reached_exits_1_with_the_clients_text() {
    let copy = Copy::new("maintenance-closed");
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

#[test]
fn the_sonda_key_is_sent_as_a_bearer_token_when_there_is_one() {
    let stub = event_stub();
    let copy = sonda_copy("maintenance-key", &stub);

    let without = nobs(&copy, &["maintenance", "-d", "srl1"]);
    copy.append_env("SONDA_API_KEY=sonda-key\n");
    let with = nobs(&copy, &["maintenance", "-d", "srl1"]);

    assert_eq!(without.status.code(), Some(0), "{}", stderr(&without));
    assert_eq!(with.status.code(), Some(0), "{}", stderr(&with));
    let requests = stub.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].header("authorization"), None);
    assert_eq!(
        requests[1].header("authorization"),
        Some("Bearer sonda-key")
    );
}

#[test]
fn the_sonda_sink_variable_sets_the_sink_of_the_annotation() {
    let stub = event_stub();
    let copy = sonda_copy("maintenance-sink", &stub);
    copy.append_env("SONDA_LOKI_SINK_URL=http://sink:3100\n");

    let output = nobs(&copy, &["maintenance", "-d", "srl1"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stub.requests()[0].body,
        annotation("srl1", true, "http://sink:3100").as_bytes()
    );
}

#[test]
fn a_blank_sonda_key_or_sink_counts_as_not_given() {
    let stub = event_stub();
    let copy = sonda_copy("maintenance-blank", &stub);
    copy.append_env("SONDA_API_KEY=   \nSONDA_LOKI_SINK_URL=   \n");

    let output = nobs(&copy, &["maintenance", "-d", "srl1"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let requests = stub.requests();
    assert_eq!(requests[0].header("authorization"), None);
    assert_eq!(
        requests[0].body,
        annotation("srl1", true, DEFAULT_SINK).as_bytes()
    );
}
