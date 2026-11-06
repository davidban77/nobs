use std::ffi::OsString;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

use crate::runner::Runner;

pub const SCRIPT: &str = include_str!("../../scripts/infrahub_fallback.py");
pub const DEFAULT_ADDRESS: &str = "http://localhost:8000";

const CONTAINER_HOST: &str = "infrahub-server";
const SEED_KINDS: [&str; 3] = ["WorkshopDevice", "WorkshopInterface", "WorkshopBgpSession"];

static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

pub fn host_address(address: &str) -> String {
    if address.contains(CONTAINER_HOST) {
        DEFAULT_ADDRESS.to_string()
    } else {
        address.trim_end_matches('/').to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Uv(String),
    Script(i32),
    NotFound { kind: String, device: String },
    Body(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Uv(reason) => write!(f, "cannot run uv: {reason}"),
            Error::Script(code) => write!(f, "the Infrahub script exited {code}"),
            Error::NotFound { kind, device } => write!(f, "{kind} {device} not found in Infrahub."),
            Error::Body(reason) => write!(
                f,
                "the Infrahub script answered with text nobs cannot read: {reason}"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    pub rows: Vec<(String, u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maintenance {
    pub previous: bool,
    pub state: bool,
}

pub struct Infrahub {
    address: String,
    token: String,
}

struct TempScript(PathBuf);

impl TempScript {
    fn create() -> Result<Self, Error> {
        let number = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let name = format!("nobs-infrahub-{}-{number}.py", std::process::id());
        Self::create_at(std::env::temp_dir().join(name))
    }

    fn create_at(path: PathBuf) -> Result<Self, Error> {
        let cannot =
            |error: io::Error| Error::Uv(format!("cannot write {}: {error}", path.display()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(cannot)?;
        let script = Self(path.clone());
        file.write_all(SCRIPT.as_bytes()).map_err(cannot)?;
        Ok(script)
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Infrahub {
    pub fn new(address: &str, token: &str) -> Self {
        Self {
            address: host_address(address),
            token: token.to_string(),
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn schema(&self, runner: &dyn Runner, file: &Path) -> Result<(), Error> {
        let value = parse(&self.call(runner, &["schema".into(), file.into()])?)?;
        if flag(&value, "ok")? {
            Ok(())
        } else {
            Err(Error::Body(format!("\"ok\" is false in {value}")))
        }
    }

    pub fn seed(&self, runner: &dyn Runner, lab_vars: &Path) -> Result<Seed, Error> {
        let stdout = self.call(runner, &["seed".into(), lab_vars.into()])?;
        let value = parse(&stdout)?;
        let rows = SEED_KINDS
            .iter()
            .map(|kind| {
                Ok((
                    kind.to_string(),
                    count(&value, kind, "created")?,
                    count(&value, kind, "updated")?,
                ))
            })
            .collect::<Result<_, Error>>()?;
        Ok(Seed { rows })
    }

    pub fn maintenance(
        &self,
        runner: &dyn Runner,
        kind: &str,
        device: &str,
        state: bool,
    ) -> Result<Maintenance, Error> {
        let words: [OsString; 5] = [
            "maintenance".into(),
            device.into(),
            state.to_string().into(),
            "--kind".into(),
            kind.into(),
        ];
        let value = parse(&self.call(runner, &words)?)?;
        if !flag(&value, "found")? {
            return Err(Error::NotFound {
                kind: kind.to_string(),
                device: device.to_string(),
            });
        }
        Ok(Maintenance {
            previous: flag(&value, "previous")?,
            state: flag(&value, "state")?,
        })
    }

    fn call(&self, runner: &dyn Runner, words: &[OsString]) -> Result<String, Error> {
        let script = TempScript::create()?;
        let mut args: Vec<OsString> = vec![
            "run".into(),
            "--quiet".into(),
            "--script".into(),
            script.0.clone().into_os_string(),
        ];
        args.extend_from_slice(words);
        let env = [
            ("INFRAHUB_ADDRESS".to_string(), self.address.clone()),
            ("INFRAHUB_API_TOKEN".to_string(), self.token.clone()),
        ];
        let output = runner
            .output_env("uv", &args, &env)
            .map_err(|error| Error::Uv(error.to_string()))?;
        if output.code != 0 {
            return Err(Error::Script(output.code));
        }
        Ok(output.stdout)
    }
}

fn parse(stdout: &str) -> Result<Value, Error> {
    serde_json::from_str(stdout).map_err(|error| Error::Body(error.to_string()))
}

fn flag(value: &Value, key: &str) -> Result<bool, Error> {
    value
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| Error::Body(format!("no true or false \"{key}\" in {value}")))
}

fn count(value: &Value, kind: &str, field: &str) -> Result<u32, Error> {
    value
        .get(kind)
        .and_then(|counts| counts.get(field))
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| Error::Body(format!("no {kind} {field} count in {value}")))
}

fn quoted(text: &str) -> String {
    let mut escaped = String::new();
    for character in Value::String(text.to_string()).to_string().chars() {
        if character.is_ascii() && character != '\u{7f}' {
            escaped.push(character);
            continue;
        }
        for unit in character.encode_utf16(&mut [0; 2]) {
            escaped.push_str(&format!("\\u{unit:04x}"));
        }
    }
    escaped
}

pub fn annotation(device: &str, state: bool, sink: &str) -> String {
    let flag = if state { "True" } else { "False" };
    let message = quoted(&format!(
        "Configured from CLI: {device}.maintenance = {flag}"
    ));
    let device = quoted(device);
    let sink = quoted(sink.trim_end_matches('/'));
    format!(
        r#"{{"signal_type": "logs", "labels": {{"device": {device}, "source": "workshop-trigger", "event": "config-push", "level": "info"}}, "log": {{"severity": "info", "message": {message}, "fields": {{}}}}, "encoder": {{"type": "json_lines"}}, "sink": {{"type": "loki", "url": {sink}}}}}"#
    )
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;
    use crate::runner::{Call, FakeRunner, Output};

    const SEED_REPLY: &str = r#"{"WorkshopBgpSession": {"created": 5, "updated": 6}, "WorkshopDevice": {"created": 1, "updated": 2}, "WorkshopInterface": {"created": 3, "updated": 4}}"#;

    struct Spy {
        fail: bool,
        code: i32,
        held: RefCell<Option<(PathBuf, Option<String>)>>,
        mode: Cell<Option<u32>>,
    }

    impl Spy {
        fn new(fail: bool, code: i32) -> Self {
            Self {
                fail,
                code,
                held: RefCell::new(None),
                mode: Cell::new(None),
            }
        }

        fn assert_script_was_there_and_is_gone(&self) {
            let (path, content) = self.held.take().expect("the runner was called");
            assert_eq!(content.as_deref(), Some(SCRIPT));
            assert!(!path.exists(), "{} was kept", path.display());
        }
    }

    impl Runner for Spy {
        fn run(&self, _: &str, _: &[OsString]) -> io::Result<i32> {
            unreachable!("the Infrahub calls use output_env")
        }

        fn output(&self, _: &str, _: &[OsString]) -> io::Result<Output> {
            unreachable!("the Infrahub calls use output_env")
        }

        fn output_env(
            &self,
            _: &str,
            args: &[OsString],
            _: &[(String, String)],
        ) -> io::Result<Output> {
            let path = PathBuf::from(&args[3]);
            let content = fs::read_to_string(&path).ok();
            self.mode.set(
                fs::metadata(&path)
                    .ok()
                    .map(|meta| meta.permissions().mode() & 0o777),
            );
            *self.held.borrow_mut() = Some((path, content));
            if self.fail {
                return Err(io::Error::other("no uv"));
            }
            Ok(Output {
                code: self.code,
                stdout: r#"{"ok": true}"#.to_string(),
            })
        }
    }

    fn infrahub() -> Infrahub {
        Infrahub::new("http://infrahub-server:8000", "secret")
    }

    fn texts(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn only_call(runner: &FakeRunner) -> Call {
        let mut calls = runner.outputs();
        assert_eq!(calls.len(), 1);
        calls.remove(0)
    }

    #[test]
    fn host_address_swaps_the_container_name_for_localhost() {
        assert_eq!(
            host_address("http://infrahub-server:8000"),
            "http://localhost:8000"
        );
        assert_eq!(
            host_address("http://infrahub-server:8000/"),
            "http://localhost:8000"
        );
    }

    #[test]
    fn host_address_gives_localhost_8000_whatever_port_the_container_name_had() {
        assert_eq!(
            host_address("http://infrahub-server:9999"),
            "http://localhost:8000"
        );
    }

    #[test]
    fn host_address_returns_any_other_address_unchanged() {
        assert_eq!(host_address("http://10.0.0.5:8000"), "http://10.0.0.5:8000");
        assert_eq!(
            host_address("http://localhost:8000"),
            "http://localhost:8000"
        );
    }

    #[test]
    fn host_address_drops_the_trailing_slash_of_an_address_it_does_not_rewrite() {
        assert_eq!(
            host_address("http://10.0.0.5:8000/"),
            "http://10.0.0.5:8000"
        );
    }

    #[test]
    fn new_keeps_the_host_form_of_the_address() {
        assert_eq!(infrahub().address(), "http://localhost:8000");
        assert_eq!(
            Infrahub::new("http://10.0.0.5:8000/", "t").address(),
            "http://10.0.0.5:8000"
        );
    }

    #[test]
    fn schema_runs_uv_with_the_script_the_file_and_the_host_address_and_token() {
        let runner = FakeRunner::answering(r#"{"ok": true}"#);

        infrahub()
            .schema(&runner, Path::new("/w/schema.yml"))
            .unwrap();

        let call = only_call(&runner);
        assert_eq!(call.program, "uv");
        let args = texts(&call.args);
        assert_eq!(args[..3], ["run", "--quiet", "--script"]);
        assert_eq!(args[4..], ["schema", "/w/schema.yml"]);
        assert_eq!(
            call.env,
            vec![
                (
                    "INFRAHUB_ADDRESS".to_string(),
                    "http://localhost:8000".to_string()
                ),
                ("INFRAHUB_API_TOKEN".to_string(), "secret".to_string()),
            ]
        );
    }

    #[test]
    fn schema_is_ok_only_when_the_script_printed_ok_true() {
        let ok = FakeRunner::answering(r#"{"ok": true}"#);
        assert_eq!(infrahub().schema(&ok, Path::new("s.yml")), Ok(()));
        for text in ["not json", "{}", r#"{"ok": false}"#, ""] {
            let runner = FakeRunner::answering(text);
            assert!(
                matches!(
                    infrahub().schema(&runner, Path::new("s.yml")),
                    Err(Error::Body(_))
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn seed_runs_uv_with_the_lab_vars_file() {
        let runner = FakeRunner::answering(SEED_REPLY);

        infrahub()
            .seed(&runner, Path::new("/w/lab_vars.yml"))
            .unwrap();

        let args = texts(&only_call(&runner).args);
        assert_eq!(args[4..], ["seed", "/w/lab_vars.yml"]);
    }

    #[test]
    fn maintenance_runs_uv_with_the_device_the_state_and_the_kind() {
        let runner = FakeRunner::answering(r#"{"found": true, "previous": false, "state": true}"#);

        infrahub()
            .maintenance(&runner, "WorkshopDevice", "srl1", true)
            .unwrap();
        infrahub()
            .maintenance(&runner, "Other", "srl2", false)
            .unwrap();

        let calls = runner.outputs();
        assert_eq!(
            texts(&calls[0].args)[4..],
            ["maintenance", "srl1", "true", "--kind", "WorkshopDevice"]
        );
        assert_eq!(
            texts(&calls[1].args)[4..],
            ["maintenance", "srl2", "false", "--kind", "Other"]
        );
    }

    #[test]
    fn the_temporary_file_holds_the_script_during_the_call_and_is_gone_after_it() {
        let spy = Spy::new(false, 0);
        infrahub().schema(&spy, Path::new("s.yml")).unwrap();
        spy.assert_script_was_there_and_is_gone();
    }

    #[test]
    fn the_temporary_file_can_be_read_and_written_by_its_owner_only_during_the_call() {
        let spy = Spy::new(false, 0);
        infrahub().schema(&spy, Path::new("s.yml")).unwrap();
        assert_eq!(spy.mode.get(), Some(0o600));
    }

    #[test]
    fn a_file_that_already_has_the_path_is_an_error_and_is_left_unchanged() {
        let path = std::env::temp_dir().join(format!(
            "nobs-infrahub-test-{}-existing.py",
            std::process::id()
        ));
        fs::write(&path, "not the script").unwrap();

        let result = TempScript::create_at(path.clone());

        let kept = fs::read_to_string(&path);
        let _ = fs::remove_file(&path);
        match result.err() {
            Some(Error::Uv(reason)) => {
                assert!(
                    reason.starts_with(&format!("cannot write {}: ", path.display())),
                    "{reason}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(kept.unwrap(), "not the script");
    }

    #[test]
    fn the_temporary_file_is_gone_after_a_failing_uv() {
        let spy = Spy::new(true, 0);
        assert!(matches!(
            infrahub().schema(&spy, Path::new("s.yml")),
            Err(Error::Uv(_))
        ));
        spy.assert_script_was_there_and_is_gone();
    }

    #[test]
    fn the_temporary_file_is_gone_after_a_non_zero_exit() {
        let spy = Spy::new(false, 1);
        assert_eq!(
            infrahub().schema(&spy, Path::new("s.yml")),
            Err(Error::Script(1))
        );
        spy.assert_script_was_there_and_is_gone();
    }

    #[test]
    fn two_calls_in_one_process_never_share_a_temporary_file() {
        let runner = FakeRunner::answering(r#"{"ok": true}"#);
        infrahub().schema(&runner, Path::new("a.yml")).unwrap();
        infrahub().schema(&runner, Path::new("b.yml")).unwrap();

        let calls = runner.outputs();
        let first = PathBuf::from(&calls[0].args[3]);
        let second = PathBuf::from(&calls[1].args[3]);
        assert_ne!(first, second);
        for path in [first, second] {
            assert_eq!(path.parent(), Some(std::env::temp_dir().as_path()));
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let prefix = format!("nobs-infrahub-{}-", std::process::id());
            assert!(name.starts_with(&prefix) && name.ends_with(".py"), "{name}");
        }
    }

    #[test]
    fn seed_reads_the_counts_by_kind_in_the_workshop_order() {
        let runner = FakeRunner::answering(SEED_REPLY);

        let seed = infrahub().seed(&runner, Path::new("l.yml")).unwrap();

        assert_eq!(
            seed.rows,
            vec![
                ("WorkshopDevice".to_string(), 1, 2),
                ("WorkshopInterface".to_string(), 3, 4),
                ("WorkshopBgpSession".to_string(), 5, 6),
            ]
        );
    }

    #[test]
    fn seed_without_the_counts_of_a_kind_is_a_body_error() {
        let runner = FakeRunner::answering(r#"{"WorkshopDevice": {"created": 1, "updated": 2}}"#);
        assert!(matches!(
            infrahub().seed(&runner, Path::new("l.yml")),
            Err(Error::Body(_))
        ));
    }

    #[test]
    fn maintenance_reads_the_previous_value_and_the_new_state() {
        let runner = FakeRunner::answering(r#"{"found": true, "previous": false, "state": true}"#);
        assert_eq!(
            infrahub().maintenance(&runner, "WorkshopDevice", "srl1", true),
            Ok(Maintenance {
                previous: false,
                state: true
            })
        );
    }

    #[test]
    fn maintenance_for_a_device_infrahub_does_not_have_is_not_found() {
        let runner = FakeRunner::answering(r#"{"found": false}"#);
        assert_eq!(
            infrahub().maintenance(&runner, "WorkshopDevice", "srl9", true),
            Err(Error::NotFound {
                kind: "WorkshopDevice".to_string(),
                device: "srl9".to_string()
            })
        );
    }

    #[test]
    fn a_non_zero_exit_is_the_script_error_with_its_code() {
        let runner = FakeRunner::returning(1);
        assert_eq!(
            infrahub().maintenance(&runner, "WorkshopDevice", "srl1", true),
            Err(Error::Script(1))
        );
        assert_eq!(
            infrahub().seed(&runner, Path::new("l.yml")),
            Err(Error::Script(1))
        );
    }

    #[test]
    fn a_uv_that_cannot_run_is_a_uv_error() {
        let runner = FakeRunner::failing();
        match infrahub().seed(&runner, Path::new("l.yml")) {
            Err(Error::Uv(reason)) => assert_eq!(reason, "fake runner failure"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn text_that_is_not_json_is_a_body_error() {
        let runner = FakeRunner::answering("Traceback (most recent call last)");
        assert!(matches!(
            infrahub().seed(&runner, Path::new("l.yml")),
            Err(Error::Body(_))
        ));
        assert!(matches!(
            infrahub().maintenance(&runner, "WorkshopDevice", "srl1", true),
            Err(Error::Body(_))
        ));
    }

    #[test]
    fn the_error_texts_are_the_ones_the_commands_print() {
        assert_eq!(
            Error::Uv("no such file".to_string()).to_string(),
            "cannot run uv: no such file"
        );
        assert_eq!(Error::Script(3).to_string(), "the Infrahub script exited 3");
        assert_eq!(
            Error::NotFound {
                kind: "WorkshopDevice".to_string(),
                device: "srl1".to_string()
            }
            .to_string(),
            "WorkshopDevice srl1 not found in Infrahub."
        );
        assert_eq!(
            Error::Body("expected value".to_string()).to_string(),
            "the Infrahub script answered with text nobs cannot read: expected value"
        );
    }

    #[test]
    fn no_call_changes_the_environment_of_nobs() {
        let address = std::env::var_os("INFRAHUB_ADDRESS");
        let token = std::env::var_os("INFRAHUB_API_TOKEN");
        for runner in [
            FakeRunner::answering(SEED_REPLY),
            FakeRunner::failing(),
            FakeRunner::returning(1),
        ] {
            let _ = infrahub().schema(&runner, Path::new("s.yml"));
            assert_eq!(std::env::var_os("INFRAHUB_ADDRESS"), address);
            let _ = infrahub().seed(&runner, Path::new("l.yml"));
            assert_eq!(std::env::var_os("INFRAHUB_ADDRESS"), address);
            let _ = infrahub().maintenance(&runner, "WorkshopDevice", "srl1", true);
            assert_eq!(std::env::var_os("INFRAHUB_ADDRESS"), address);
            assert_eq!(std::env::var_os("INFRAHUB_API_TOKEN"), token);
        }
    }

    fn python_body(state: &str) -> Value {
        json!({
            "signal_type": "logs",
            "labels": {
                "device": "srl1",
                "source": "workshop-trigger",
                "event": "config-push",
                "level": "info"
            },
            "log": {
                "severity": "info",
                "message": format!("Configured from CLI: srl1.maintenance = {state}"),
                "fields": {}
            },
            "encoder": {"type": "json_lines"},
            "sink": {"type": "loki", "url": "http://loki:3001"}
        })
    }

    #[test]
    fn annotation_is_the_body_the_python_sends() {
        let text = annotation("srl1", true, "http://loki:3001");
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            python_body("True")
        );
        assert_eq!(
            text,
            r#"{"signal_type": "logs", "labels": {"device": "srl1", "source": "workshop-trigger", "event": "config-push", "level": "info"}, "log": {"severity": "info", "message": "Configured from CLI: srl1.maintenance = True", "fields": {}}, "encoder": {"type": "json_lines"}, "sink": {"type": "loki", "url": "http://loki:3001"}}"#
        );
    }

    #[test]
    fn annotation_for_false_says_false() {
        let text = annotation("srl1", false, "http://loki:3001");
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            python_body("False")
        );
        assert!(text.contains("srl1.maintenance = False"));
    }

    #[test]
    fn annotation_drops_the_trailing_slash_of_the_sink() {
        let text = annotation("srl1", true, "http://loki:3001/");
        assert!(text.ends_with(r#""url": "http://loki:3001"}}"#), "{text}");
    }

    #[test]
    fn annotation_writes_every_non_ascii_character_as_utf16_escapes_like_python() {
        let text = annotation("é😀\u{7f}", true, "http://loki:3001");

        assert!(text.is_ascii(), "{text}");
        let escaped: String = [0xe9_u16, 0xd83d, 0xde00, 0x7f]
            .iter()
            .map(|unit| format!("{}u{unit:04x}", '\\'))
            .collect();
        assert!(
            text.contains(&format!(r#""device": "{escaped}""#)),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                r#""message": "Configured from CLI: {escaped}.maintenance = True""#
            )),
            "{text}"
        );
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["labels"]["device"], "é😀\u{7f}");
    }

    #[test]
    fn annotation_escapes_the_device_name_wherever_it_appears() {
        let text = annotation("a\"b", true, "http://loki:3001");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["labels"]["device"], "a\"b");
        assert_eq!(
            value["log"]["message"],
            "Configured from CLI: a\"b.maintenance = True"
        );
    }
}
