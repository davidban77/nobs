use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "../../tests/support/stub.rs"]
mod stub;

use stub::Stub;

const ALERTS: &str = "/api/v2/alerts?active=true&silenced=true&inhibited=true";
const SILENCES: &str = "/api/v2/silences";
const HEADER: [&str; 5] = ["Alertname", "Severity", "Device / target", "State", "Age"];

struct Copy(PathBuf);

impl Copy {
    fn new(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-alerts-{name}"));
        let _ = fs::remove_dir_all(&root);
        copy_tree(&fixture(), &root.join("nobs-t-one"));
        Self(root)
    }

    fn dir(&self) -> PathBuf {
        self.0.join("nobs-t-one")
    }

    fn write_env(&self, text: &str) {
        fs::write(self.dir().join(".env"), text).unwrap();
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

fn nobs(copy: &Copy, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nobs"))
        .arg("nobs-t-one")
        .args(args)
        .env_remove("ALERTMANAGER_URL")
        .env_remove("NOBS_DIR")
        .current_dir(copy.dir())
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn cells(line: &str) -> Vec<&str> {
    line.split("  ")
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .collect()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86400) + 719_468;
    let rest = seconds.rem_euclid(86400);
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn two_alerts() -> String {
    let started = rfc3339(now() - 20);
    format!(
        r#"[
            {{"labels":{{"alertname":"BgpSessionNotUp","severity":"critical","device":"srl1","peer_address":"10.0.0.2"}},"status":{{"state":"active","silencedBy":[],"inhibitedBy":[]}},"startsAt":"{started}"}},
            {{"labels":{{"alertname":"InterfaceDown","severity":"warning","device":"srl2","interface":"ethernet-1/1"}},"status":{{"state":"suppressed","silencedBy":["s1"],"inhibitedBy":[]}},"startsAt":"{started}"}}
        ]"#
    )
}

fn alert_stub() -> Stub {
    Stub::start(&[("GET", ALERTS, 200, &two_alerts())])
}

#[test]
fn alerts_prints_one_row_per_alert_and_a_suppressed_alert_with_a_silence_reads_silenced() {
    let copy = Copy::new("two-alerts");
    let stub = alert_stub();

    let output = nobs(&copy, &["alerts", "--am-url", stub.url()]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let out = stdout(&output);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    assert_eq!(cells(lines[0]), HEADER);
    let first = cells(lines[1]);
    let second = cells(lines[2]);
    assert_eq!(
        first[..4],
        ["BgpSessionNotUp", "critical", "srl1 → 10.0.0.2", "firing"]
    );
    assert_eq!(
        second[..4],
        [
            "InterfaceDown",
            "warning",
            "srl2 → ethernet-1/1",
            "silenced"
        ]
    );
    for age in [first[4], second[4]] {
        let seconds: i64 = age.strip_suffix('s').unwrap().parse().unwrap();
        assert!((20..60).contains(&seconds), "{age}");
    }
    let requests = stub.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, ALERTS);
}

#[test]
fn a_label_keeps_only_the_alerts_that_have_it() {
    let copy = Copy::new("one-label");
    let stub = alert_stub();

    let output = nobs(
        &copy,
        &[
            "alerts",
            "--am-url",
            stub.url(),
            "-l",
            "alertname=BgpSessionNotUp",
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let out = stdout(&output);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert_eq!(cells(lines[1])[0], "BgpSessionNotUp");
}

#[test]
fn every_given_label_must_match_and_the_key_and_the_value_are_trimmed() {
    let copy = Copy::new("two-labels");
    let stub = alert_stub();

    let both = nobs(
        &copy,
        &[
            "alerts",
            "--am-url",
            stub.url(),
            "--label",
            " alertname = BgpSessionNotUp ",
            "--label",
            "device=srl1",
        ],
    );
    let split = nobs(
        &copy,
        &[
            "alerts",
            "--am-url",
            stub.url(),
            "-l",
            "alertname=BgpSessionNotUp",
            "-l",
            "device=srl2",
        ],
    );

    let out = stdout(&both);
    assert_eq!(out.lines().count(), 2, "{out}");
    assert_eq!(cells(out.lines().nth(1).unwrap())[0], "BgpSessionNotUp");
    assert_eq!(
        stdout(&split),
        "Alertname  Severity  Device / target  State  Age\nNo alerts match.\n"
    );
    assert_eq!(split.status.code(), Some(0), "{}", stderr(&split));
}

#[test]
fn when_a_key_is_given_twice_the_last_value_counts() {
    let copy = Copy::new("repeated-key");
    let stub = alert_stub();

    let output = nobs(
        &copy,
        &[
            "alerts",
            "--am-url",
            stub.url(),
            "-l",
            "device=srl1",
            "-l",
            "device=srl2",
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let out = stdout(&output);
    assert_eq!(out.lines().count(), 2, "{out}");
    assert_eq!(cells(out.lines().nth(1).unwrap())[0], "InterfaceDown");
}

#[test]
fn no_alert_left_prints_the_header_and_no_alerts_match_and_exits_0() {
    let copy = Copy::new("no-match");
    let stub = alert_stub();

    let output = nobs(
        &copy,
        &["alerts", "--am-url", stub.url(), "-l", "device=srl9"],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "Alertname  Severity  Device / target  State  Age\nNo alerts match.\n"
    );
}

#[test]
fn a_label_without_an_equals_sign_exits_2_and_sends_nothing() {
    let copy = Copy::new("bad-label");
    let stub = alert_stub();

    let output = nobs(
        &copy,
        &[
            "alerts",
            "--am-url",
            stub.url(),
            "-l",
            "device=srl1",
            "-l",
            "bad",
        ],
    );

    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    assert_eq!(
        stderr(&output),
        "nobs: --label must be key=value (got 'bad')\n"
    );
    assert_eq!(stdout(&output), "");
    assert!(stub.requests().is_empty());
}

#[test]
fn no_silenced_sends_silenced_false() {
    let copy = Copy::new("no-silenced");
    let path = "/api/v2/alerts?active=true&silenced=false&inhibited=true";
    let stub = Stub::start(&[("GET", path, 200, "[]")]);

    let output = nobs(&copy, &["alerts", "--am-url", stub.url(), "--no-silenced"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let requests = stub.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, path);
}

#[test]
fn the_last_flag_of_a_pair_wins() {
    let copy = Copy::new("last-wins");
    let path = "/api/v2/alerts?active=true&silenced=false&inhibited=false";
    let stub = Stub::start(&[("GET", path, 200, "[]")]);

    let output = nobs(
        &copy,
        &[
            "alerts",
            "--am-url",
            stub.url(),
            "--silenced",
            "--no-silenced",
            "--inhibited",
            "--no-inhibited",
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stub.requests()[0].path, path);
}

#[test]
fn the_dot_env_file_gives_the_address_when_there_is_no_flag() {
    let copy = Copy::new("dot-env");
    let stub = alert_stub();
    copy.write_env(&format!("ALERTMANAGER_URL={}\n", stub.url()));

    let output = nobs(&copy, &["alerts"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output).lines().count(), 3);
    assert_eq!(stub.requests().len(), 1);
}

#[test]
fn a_closed_port_exits_1_and_says_it_cannot_reach_alertmanager() {
    let copy = Copy::new("closed-port");
    let closed = closed_port_url();

    let output = nobs(&copy, &["alerts", "--am-url", &closed]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with(&format!("nobs: cannot reach Alertmanager at {closed}: ")),
        "{}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "");
}

#[test]
fn a_closed_port_makes_silences_exit_1_and_say_it_cannot_reach_alertmanager() {
    let copy = Copy::new("silences-closed-port");
    let closed = closed_port_url();

    let output = nobs(&copy, &["silences", "--am-url", &closed]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with(&format!("nobs: cannot reach Alertmanager at {closed}: ")),
        "{}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "");
}

fn silences_body(now: i64) -> String {
    let starts = rfc3339(now - 3600);
    let ends = rfc3339(now + 3600);
    let past = rfc3339(now - 60);
    format!(
        r#"[
            {{"id":"0a1b2c3d-4e5f-6789-abcd","status":{{"state":"active"}},"startsAt":"{starts}","endsAt":"{ends}","matchers":[{{"name":"alertname","value":"BgpSessionNotUp","isRegex":false}},{{"name":"device","value":"srl1","isRegex":false}}],"createdBy":"alice-the-very-long"}},
            {{"id":"ffffffff-0000-1111-2222","status":{{"state":"expired"}},"startsAt":"{starts}","endsAt":"{past}","matchers":[{{"name":"device","value":"srl2","isRegex":false}}],"createdBy":"bob"}}
        ]"#
    )
}

#[test]
fn silences_leaves_out_expired_ones_and_prints_the_columns_of_the_rest() {
    let copy = Copy::new("silences");
    let now = now();
    let body = silences_body(now);
    let stub = Stub::start(&[("GET", SILENCES, 200, &body)]);

    let output = nobs(&copy, &["silences", "--am-url", stub.url()]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let out = stdout(&output);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert_eq!(
        cells(lines[0]),
        [
            "ID",
            "State",
            "Starts (UTC)",
            "Ends (UTC)",
            "Remaining",
            "Matchers",
            "Creator"
        ]
    );
    let row = cells(lines[1]);
    assert_eq!(row.len(), 7, "{out}");
    assert_eq!(row[..2], ["0a1b2c3d-4e5…", "active"]);
    assert_eq!(row[2], &rfc3339(now - 3600)[11..19]);
    assert_eq!(row[3], &rfc3339(now + 3600)[11..19]);
    assert!(row[4].contains('m') && row[4].ends_with('s'), "{}", row[4]);
    assert_eq!(
        row[5..],
        ["alertname=BgpSessionNotUp, device=srl1", "alice-the-very"]
    );
}

#[test]
fn show_expired_adds_the_expired_silences() {
    let copy = Copy::new("show-expired");
    let body = silences_body(now());
    let stub = Stub::start(&[("GET", SILENCES, 200, &body)]);

    let output = nobs(
        &copy,
        &["silences", "--am-url", stub.url(), "--show-expired"],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let out = stdout(&output);
    assert_eq!(out.lines().count(), 3, "{out}");
    let expired = cells(out.lines().nth(2).unwrap());
    assert_eq!(expired[..2], ["ffffffff-000…", "expired"]);
    assert_eq!(expired[4], "—");
}

#[test]
fn no_silence_prints_one_line_with_the_address_and_exits_0() {
    let copy = Copy::new("no-silence");
    let stub = Stub::start(&[("GET", SILENCES, 200, "[]")]);
    let url = stub.url();

    let plain = nobs(&copy, &["silences", "--am-url", url]);
    let any = nobs(&copy, &["silences", "--am-url", url, "--show-expired"]);

    assert_eq!(plain.status.code(), Some(0), "{}", stderr(&plain));
    assert_eq!(
        stdout(&plain),
        format!("No silences active or pending in Alertmanager at {url}.\n")
    );
    assert_eq!(any.status.code(), Some(0), "{}", stderr(&any));
    assert_eq!(
        stdout(&any),
        format!("No silences (any state) in Alertmanager at {url}.\n")
    );
}
