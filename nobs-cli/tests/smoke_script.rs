use std::ffi::OsString;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const SERVICES: [&str; 6] = [
    "prefect-flows",
    "webhook",
    "alertmanager",
    "prometheus",
    "loki",
    "infrahub-server",
];

const ALERT_HEADER: &str = "Alertname  Severity  Device / target  State  Age";

const SCHEMA_POST: &str = r#"-s -o /dev/null -w %{http_code} --max-time 5 -X POST -H Content-Type: application/json -H X-INFRAHUB-KEY: t -d {"schemas":[{"version":"1.0"}]} http://localhost:8000/api/schema/load?branch=main"#;

struct Fixture {
    dir: PathBuf,
    port: u16,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("smoke-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let fixture = Fixture {
            dir,
            port: free_port(),
        };
        fixture.write_nobs(false);
        write_executable(
            &fixture.dir.join("bin/curl"),
            &format!(
                "#!/bin/sh\n\
                 echo \"$*\" >> '{log}'\n\
                 for arg in \"$@\"; do\n\
                 if [ \"$arg\" = \"-w\" ]; then\n\
                 if [ -f http-code ]; then cat http-code; else printf 200; fi\n\
                 fi\n\
                 done\n\
                 if [ -f exit-curl ]; then exit \"$(cat exit-curl)\"; fi\n\
                 exit 0\n",
                log = fixture.dir.join("curl.log").display()
            ),
        );
        write_executable(
            &fixture.dir.join("bin/docker"),
            "#!/bin/sh\n\
             kind=\"$1\"\n\
             case \"$*\" in *--filter*) ;; *) kind=\"all-$1\" ;; esac\n\
             case \"$kind\" in\n\
             ps) file=containers ;;\n\
             volume) file=volumes ;;\n\
             network) file=networks ;;\n\
             all-ps) file=all-containers ;;\n\
             all-network) file=all-networks ;;\n\
             esac\n\
             if [ -f \"$file\" ]; then cat \"$file\"; fi\n\
             if [ -f \"exit-docker-$kind\" ]; then exit \"$(cat \"exit-docker-$kind\")\"; fi\n\
             if [ -f exit-docker ]; then exit \"$(cat exit-docker)\"; fi\n\
             exit 0\n",
        );
        fixture
    }

    fn write_nobs(&self, record_token: bool) {
        let token = if record_token {
            format!(
                "echo \"token=[$(printenv INFRAHUB_API_TOKEN)]\" >> '{}'\n",
                self.dir.join("token.log").display()
            )
        } else {
            String::new()
        };
        write_executable(
            &self.dir.join("bin/nobs"),
            &format!(
                "#!/bin/sh\n\
                 echo \"$*\" >> '{calls}'\n\
                 {token}\
                 if [ -f \"sleep-$2\" ]; then sleep \"$(cat \"sleep-$2\")\"; fi\n\
                 if [ -f \"print-$2\" ]; then cat \"print-$2\"; fi\n\
                 if [ -f \"exit-$2\" ]; then exit \"$(cat \"exit-$2\")\"; fi\n\
                 exit 0\n",
                calls = self.dir.join("calls.log").display()
            ),
        );
    }

    fn workshop(&self, name: &str, project: &str) {
        let dir = self.dir.join("workshops").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("nobs.toml"),
            format!("project = \"{project}\"\nname = \"{name}\"\nnetwork = \"{project}-net\"\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("docker-compose.yml"),
            "services:\n  web:\n    container_name: fixed-web\n  db:\n    container_name: \"fixed-db\"\n  old:\n    # container_name: fixed-old\n",
        )
        .unwrap();
        std::fs::write(dir.join(".env"), "INFRAHUB_API_TOKEN=t\n").unwrap();
        std::fs::write(
            dir.join("print-alerts"),
            format!("{ALERT_HEADER}\nBgpSessionNotUp  critical  r1  firing  3m\n"),
        )
        .unwrap();
    }

    fn set(&self, workshop: &str, file: &str, content: &str) {
        std::fs::write(
            self.dir.join("workshops").join(workshop).join(file),
            content,
        )
        .unwrap();
    }

    fn command(&self) -> Command {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/smoke.sh");
        let mut command = Command::new("/bin/bash");
        command
            .arg(script)
            .env("NOBS", self.dir.join("bin/nobs"))
            .env("SMOKE_SLEEP", "0")
            .env("SMOKE_PORTS", self.port.to_string())
            .env("PATH", path_with(&self.dir.join("bin")))
            .env_remove("INFRAHUB_API_TOKEN")
            .env_remove("SMOKE_ALERT");
        command
    }

    fn run(&self, workshops: &[&str]) -> Output {
        self.command()
            .arg(&self.dir)
            .args(workshops)
            .output()
            .unwrap()
    }

    fn read(&self, file: &str) -> Vec<String> {
        std::fs::read_to_string(self.dir.join(file))
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    }

    fn calls(&self) -> Vec<String> {
        self.read("calls.log")
    }

    fn has_no_calls(&self) -> bool {
        !self.dir.join("calls.log").exists()
    }

    fn assert_refused(&self, out: &Output, line: &str) {
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stdout(out).contains(line),
            "missing {line:?} in {}",
            stdout(out)
        );
        assert!(
            lines(out)
                .iter()
                .any(|line| timed(line, "smoke: one: failed in ")),
            "no failed in line in {}",
            stdout(out)
        );
        assert!(self.has_no_calls());
    }

    fn term_when_called(&self, call: &str) -> Option<i32> {
        let mut child = self
            .command()
            .arg(&self.dir)
            .arg("one")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while !self.dir.join("calls.log").exists() || !self.calls().contains(&call.to_string()) {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "no {call} call"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let killed = Command::new("/bin/kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap();
        assert!(killed.success());
        child.wait().unwrap().code()
    }
}

fn write_executable(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn path_with(dir: &Path) -> OsString {
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn lines(out: &Output) -> Vec<String> {
    stdout(out).lines().map(String::from).collect()
}

fn get(url: &str) -> String {
    format!("-sfo /dev/null --max-time 3 {url}")
}

fn calls_of(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn logs_calls() -> Vec<String> {
    SERVICES
        .iter()
        .map(|service| format!("one logs --no-follow --tail 100 {service}"))
        .collect()
}

fn green_calls() -> Vec<String> {
    calls_of(&[
        "one setup",
        "one up",
        "one load-infrahub",
        "one alerts -l alertname=BgpSessionNotUp",
        "one try-it --auto",
        "one destroy",
    ])
}

fn timed(line: &str, prefix: &str) -> bool {
    line.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix('s'))
        .is_some_and(|seconds| seconds.parse::<u32>().is_ok())
}

#[test]
fn a_green_run_passes_and_runs_every_step_in_order() {
    let f = Fixture::new("green");
    f.workshop("one", "nobs-t-smoke");
    let out = f.run(&["one"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(
        lines(&out).last().map(String::as_str),
        Some("smoke: 1 of 1 workshops ok")
    );
    assert_eq!(f.calls(), green_calls());
    let mut expected = Vec::new();
    for url in [
        "http://localhost:9090/-/ready",
        "http://localhost:3001/ready",
        "http://localhost:9093/-/ready",
        "http://localhost:8000/health",
    ] {
        expected.push(get(url));
        expected.push(get(url));
    }
    expected.push(SCHEMA_POST.to_string());
    assert_eq!(f.read("curl.log"), expected);
}

#[test]
fn containers_of_the_project_refuse_the_start() {
    let f = Fixture::new("containers");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "containers", "nobs-t-smoke-web-1\n");
    let out = f.run(&["one"]);
    f.assert_refused(
        &out,
        "smoke: one: containers of project nobs-t-smoke exist (nobs-t-smoke-web-1); refusing to start",
    );
}

#[test]
fn volumes_of_the_project_refuse_the_start() {
    let f = Fixture::new("volumes");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "volumes", "nobs-t-smoke_data\n");
    let out = f.run(&["one"]);
    f.assert_refused(
        &out,
        "smoke: one: volumes of project nobs-t-smoke exist (nobs-t-smoke_data); refusing to start",
    );
}

#[test]
fn networks_of_the_project_refuse_the_start() {
    let f = Fixture::new("networks");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "networks", "nobs-t-smoke_default\n");
    let out = f.run(&["one"]);
    f.assert_refused(
        &out,
        "smoke: one: networks of project nobs-t-smoke exist (nobs-t-smoke_default); refusing to start",
    );
}

#[test]
fn a_docker_listing_that_fails_refuses_the_start() {
    let f = Fixture::new("docker-fails");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "exit-docker", "1");
    let out = f.run(&["one"]);
    f.assert_refused(
        &out,
        "smoke: one: docker ps failed (exit 1); refusing to start",
    );
}

#[test]
fn a_port_in_use_refuses_the_start() {
    let mut f = Fixture::new("ports");
    f.workshop("one", "nobs-t-smoke");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    f.port = listener.local_addr().unwrap().port();
    let out = f.run(&["one"]);
    f.assert_refused(
        &out,
        &format!("smoke: one: ports in use ({}); refusing to start", f.port),
    );
}

#[test]
fn a_readiness_wait_that_never_passes_fails_dumps_the_logs_and_destroys() {
    let f = Fixture::new("never-ready");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "exit-curl", "22");
    let out = f.run(&["one"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("smoke: one: http://localhost:9090/-/ready never ready"));
    let mut expected = calls_of(&["one setup", "one up"]);
    expected.extend(logs_calls());
    expected.push("one destroy".to_string());
    assert_eq!(f.calls(), expected);
    assert_eq!(
        f.read("curl.log"),
        vec![get("http://localhost:9090/-/ready"); 61]
    );
}

#[test]
fn an_alert_wait_that_times_out_fails_dumps_the_logs_and_skips_the_tour() {
    let f = Fixture::new("alert-timeout");
    f.workshop("one", "nobs-t-smoke");
    f.set(
        "one",
        "print-alerts",
        &format!("{ALERT_HEADER}\nNo alerts match.\n"),
    );
    let out = f.run(&["one"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("smoke: one: no firing BgpSessionNotUp alert after 120s"));
    let mut expected = calls_of(&["one setup", "one up", "one load-infrahub"]);
    expected.extend(vec![
        "one alerts -l alertname=BgpSessionNotUp".to_string();
        24
    ]);
    expected.extend(logs_calls());
    expected.push("one destroy".to_string());
    assert_eq!(f.calls(), expected);
}

#[test]
fn a_failing_alerts_call_is_named_right_before_the_timeout() {
    let f = Fixture::new("alerts-fail");
    f.workshop("one", "nobs-t-smoke");
    f.set(
        "one",
        "print-alerts",
        &format!("{ALERT_HEADER}\nNo alerts match.\n"),
    );
    f.set("one", "exit-alerts", "1");
    let out = f.run(&["one"]);
    assert_eq!(out.status.code(), Some(1));
    let lines = lines(&out);
    let failed = lines
        .iter()
        .position(|line| line == "smoke: one: alerts failed (exit 1)")
        .expect("alerts failed line");
    assert_eq!(
        lines[failed + 1],
        "smoke: one: no firing BgpSessionNotUp alert after 120s"
    );
}

#[test]
fn a_failing_tour_fails_dumps_the_logs_and_destroys() {
    let f = Fixture::new("tour-fails");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "exit-try-it", "1");
    let out = f.run(&["one"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("smoke: one: try-it --auto failed (exit 1)"));
    let mut expected = green_calls();
    expected.pop();
    expected.extend(logs_calls());
    expected.push("one destroy".to_string());
    assert_eq!(f.calls(), expected);
}

#[test]
fn a_failing_destroy_fails_an_otherwise_green_run() {
    let f = Fixture::new("destroy-fails");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "exit-destroy", "1");
    let out = f.run(&["one"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("smoke: one: destroy failed (exit 1)"));
    assert!(text.contains("smoke: one: failed in"));
    assert!(!text.contains("ok in"));
    assert_eq!(f.calls(), green_calls());
}

#[test]
fn a_failing_workshop_does_not_skip_the_next_one() {
    let f = Fixture::new("two-workshops");
    f.workshop("one", "nobs-t-smoke");
    f.workshop("two", "nobs-t-smoke-two");
    f.set("one", "exit-up", "1");
    let out = f.run(&["one", "two"]);
    assert_eq!(out.status.code(), Some(1));
    let lines = lines(&out);
    let one = lines
        .iter()
        .position(|line| timed(line, "smoke: one: failed in "))
        .expect("one failed line");
    let two = lines
        .iter()
        .position(|line| timed(line, "smoke: two: ok in "))
        .expect("two ok line");
    assert!(one < two);
    assert_eq!(two + 2, lines.len());
    assert_eq!(lines[two + 1], "smoke: 1 of 2 workshops ok");
    assert!(f.calls().contains(&"two try-it --auto".to_string()));
}

#[test]
fn the_alert_name_comes_from_smoke_alert() {
    let f = Fixture::new("alert-name");
    f.workshop("one", "nobs-t-smoke");
    let out = f
        .command()
        .arg(&f.dir)
        .arg("one")
        .env("SMOKE_ALERT", "Other")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(f
        .calls()
        .contains(&"one alerts -l alertname=Other".to_string()));
    let text = stdout(&out);
    assert!(text.contains("smoke: one: wait Other alert"));
    assert!(text.contains("smoke: one: Other is firing (1 alert(s))"));
}

#[test]
fn the_token_is_read_from_env_without_quotes_and_reaches_no_child_process() {
    let cases = [
        ("plain", "INFRAHUB_API_TOKEN=t\n"),
        ("double-quoted", "INFRAHUB_API_TOKEN=\"t\"\n"),
        ("single-quoted", "INFRAHUB_API_TOKEN='t'\n"),
        (
            "two-lines",
            "INFRAHUB_API_TOKEN=a\nOTHER=b\nINFRAHUB_API_TOKEN=t\n",
        ),
    ];
    for (name, env) in cases {
        let f = Fixture::new(&format!("token-{name}"));
        f.workshop("one", "nobs-t-smoke");
        f.set("one", ".env", env);
        f.write_nobs(true);
        let out = f.run(&["one"]);
        assert_eq!(out.status.code(), Some(0), "{name}: {}", stdout(&out));
        assert_eq!(
            f.read("curl.log").last().map(String::as_str),
            Some(SCHEMA_POST),
            "{name}"
        );
        assert_eq!(f.read("token.log"), vec!["token=[]"; 6], "{name}");
    }
}

#[test]
fn a_nobs_toml_without_a_key_refuses_the_start() {
    let f = Fixture::new("toml-keys");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "nobs.toml", "name = \"one\"\n");
    let out = f.run(&["one"]);
    f.assert_refused(
        &out,
        "smoke: one: no project in nobs.toml; refusing to start",
    );
    f.set("one", "nobs.toml", "project = \"nobs-t-smoke\"\n");
    let out = f.run(&["one"]);
    f.assert_refused(&out, "smoke: one: no name in nobs.toml; refusing to start");
    f.set(
        "one",
        "nobs.toml",
        "project = \"nobs-t-smoke\"\nname = \"one\"\n",
    );
    let out = f.run(&["one"]);
    f.assert_refused(
        &out,
        "smoke: one: no network in nobs.toml; refusing to start",
    );
}

#[test]
fn containers_with_the_names_the_compose_file_fixes_refuse_the_start() {
    let f = Fixture::new("fixed-names");
    f.workshop("one", "nobs-t-smoke");
    f.set(
        "one",
        "all-containers",
        "other\nfixed-web\nfixed-web2\nfixed-db\n",
    );
    let out = f.run(&["one"]);
    f.assert_refused(
        &out,
        "smoke: one: containers named by the compose file exist (fixed-web, fixed-db); refusing to start",
    );
}

#[test]
fn a_container_named_like_a_commented_out_name_does_not_refuse_the_start() {
    let f = Fixture::new("commented-name");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "all-containers", "fixed-old\n");
    let out = f.run(&["one"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(f.calls(), green_calls());
}

#[test]
fn the_network_named_by_the_setting_refuses_the_start() {
    let f = Fixture::new("network-exists");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "all-networks", "bridge\nnobs-t-smoke-net\n");
    let out = f.run(&["one"]);
    f.assert_refused(
        &out,
        "smoke: one: network nobs-t-smoke-net exists; refusing to start",
    );
}

#[test]
fn a_plain_docker_listing_that_fails_refuses_the_start() {
    for (kind, line) in [
        (
            "all-ps",
            "smoke: one: docker ps failed (exit 1); refusing to start",
        ),
        (
            "all-network",
            "smoke: one: docker network ls failed (exit 1); refusing to start",
        ),
    ] {
        let f = Fixture::new(&format!("plain-listing-{kind}"));
        f.workshop("one", "nobs-t-smoke");
        f.set("one", &format!("exit-docker-{kind}"), "1");
        let out = f.run(&["one"]);
        f.assert_refused(&out, line);
    }
}

#[test]
fn a_relative_root_and_a_relative_nobs_work_over_two_workshops() {
    let f = Fixture::new("relative");
    f.workshop("one", "nobs-t-smoke");
    f.workshop("two", "nobs-t-smoke-two");
    let base = f.dir.parent().unwrap();
    let folder = f.dir.file_name().unwrap();
    let out = f
        .command()
        .current_dir(base)
        .arg(folder)
        .args(["one", "two"])
        .env("NOBS", Path::new(folder).join("bin/nobs"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(
        lines(&out).last().map(String::as_str),
        Some("smoke: 2 of 2 workshops ok")
    );
    assert_eq!(f.calls().len(), 12);
}

#[test]
fn a_failing_logs_call_does_not_stop_the_other_logs_or_destroy() {
    let f = Fixture::new("logs-fail");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "exit-up", "1");
    f.set("one", "exit-logs", "1");
    let out = f.run(&["one"]);
    assert_eq!(out.status.code(), Some(1));
    let mut expected = calls_of(&["one setup", "one up"]);
    expected.extend(logs_calls());
    expected.push("one destroy".to_string());
    assert_eq!(f.calls(), expected);
}

#[test]
fn no_workshop_argument_runs_autocon5_then_packt() {
    let f = Fixture::new("default-workshops");
    f.workshop("autocon5", "nobs-t-smoke-a");
    f.workshop("packt", "nobs-t-smoke-p");
    let out = f.run(&[]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert_eq!(
        lines(&out).last().map(String::as_str),
        Some("smoke: 2 of 2 workshops ok")
    );
    let mut expected: Vec<String> = green_calls()
        .iter()
        .map(|call| call.replacen("one", "autocon5", 1))
        .collect();
    expected.extend(
        green_calls()
            .iter()
            .map(|call| call.replacen("one", "packt", 1)),
    );
    assert_eq!(f.calls(), expected);
}

#[test]
fn a_missing_root_prints_the_usage_and_exits_2() {
    let f = Fixture::new("usage");
    f.workshop("one", "nobs-t-smoke");
    let out = f.command().output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stdout(&out),
        "smoke: usage: scripts/smoke.sh ROOT [WORKSHOP ...]\n"
    );
    assert!(f.has_no_calls());
}

#[test]
fn a_term_signal_destroys_the_stack_and_exits_1() {
    let f = Fixture::new("term");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "sleep-up", "2");
    assert_eq!(f.term_when_called("one up"), Some(1));
    assert_eq!(f.calls(), calls_of(&["one setup", "one up", "one destroy"]));
}

#[test]
fn a_term_signal_during_destroy_does_not_destroy_twice() {
    let f = Fixture::new("term-in-destroy");
    f.workshop("one", "nobs-t-smoke");
    f.set("one", "sleep-destroy", "2");
    assert_eq!(f.term_when_called("one destroy"), Some(1));
    assert_eq!(f.calls(), green_calls());
}
