use std::ffi::OsString;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use crate::runner::Runner;

const KIB_PER_GIB: f64 = 1024.0 * 1024.0;
#[cfg(target_os = "macos")]
const BYTES_PER_GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const MEMORY_OK_GIB: f64 = 8.0;
const MEMORY_WARN_GIB: f64 = 6.0;
const DISK_OK_GIB: f64 = 5.0;
const DEFAULT_PORT: u16 = 443;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

pub fn preflight(runner: &dyn Runner, dir: &Path, hosts: &[&str]) -> Vec<Check> {
    vec![
        tool_check(runner, "docker", &["--version"]),
        tool_check(runner, "compose", &["compose", "version"]),
        memory_check(runner),
        disk_check(runner, dir),
        network_check(hosts),
    ]
}

pub fn memory_gib(runner: &dyn Runner) -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        let output = runner
            .output("sysctl", &["-n".into(), "hw.memsize".into()])
            .ok()?;
        if output.code != 0 {
            return None;
        }
        let bytes: u64 = output.stdout.trim().parse().ok()?;
        Some(bytes as f64 / BYTES_PER_GIB)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = runner;
        parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = runner;
        None
    }
}

pub fn disk_free_gib(runner: &dyn Runner, dir: &Path) -> Option<f64> {
    let args: [OsString; 2] = ["-k".into(), dir.as_os_str().to_owned()];
    let output = runner.output("df", &args).ok()?;
    if output.code != 0 {
        return None;
    }
    let free_kib: u64 = output
        .stdout
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(free_kib as f64 / KIB_PER_GIB)
}

pub fn reachable(host: &str) -> bool {
    let target = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:{DEFAULT_PORT}")
    };
    let Some(address) = target
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
    else {
        return false;
    };
    TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).is_ok()
}

#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(text: &str) -> Option<f64> {
    let kib: u64 = text
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kib as f64 / KIB_PER_GIB)
}

fn tool_check(runner: &dyn Runner, name: &str, args: &[&str]) -> Check {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let (status, detail) = match runner.output("docker", &args) {
        Ok(output) if output.code == 0 => (
            Status::Ok,
            output
                .stdout
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
        ),
        Ok(output) => (Status::Fail, format!("exited with code {}", output.code)),
        Err(error) => (Status::Fail, error.to_string()),
    };
    check(name, status, detail)
}

fn memory_check(runner: &dyn Runner) -> Check {
    match memory_gib(runner) {
        Some(gib) => {
            let status = if gib >= MEMORY_OK_GIB {
                Status::Ok
            } else if gib >= MEMORY_WARN_GIB {
                Status::Warn
            } else {
                Status::Fail
            };
            check("memory", status, format!("{} GiB", gib.floor() as u64))
        }
        None => check("memory", Status::Warn, "memory not checked".to_string()),
    }
}

fn disk_check(runner: &dyn Runner, dir: &Path) -> Check {
    match disk_free_gib(runner, dir) {
        Some(gib) => {
            let status = if gib >= DISK_OK_GIB {
                Status::Ok
            } else {
                Status::Fail
            };
            check("disk", status, format!("{} GiB free", gib.floor() as u64))
        }
        None => check("disk", Status::Warn, "disk not checked".to_string()),
    }
}

fn network_check(hosts: &[&str]) -> Check {
    let silent: Vec<&str> = hosts
        .iter()
        .copied()
        .filter(|host| !reachable(host))
        .collect();
    if silent.is_empty() {
        check(
            "network",
            Status::Ok,
            format!("{} hosts answered", hosts.len()),
        )
    } else {
        check(
            "network",
            Status::Warn,
            format!("no answer from {}", silent.join(", ")),
        )
    }
}

fn check(name: &str, status: Status, detail: String) -> Check {
    Check {
        name: name.to_string(),
        status,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::FakeRunner;
    use std::net::TcpListener;

    #[cfg(target_os = "macos")]
    const GIB: u64 = 1024 * 1024 * 1024;

    fn find<'a>(checks: &'a [Check], name: &str) -> &'a Check {
        checks.iter().find(|check| check.name == name).unwrap()
    }

    fn closed_host() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("127.0.0.1:{}", listener.local_addr().unwrap().port())
    }

    fn disk_answer(free_kib: u64) -> String {
        format!(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s5 971350180 1000 {free_kib} 2% /\n"
        )
    }

    #[cfg(target_os = "macos")]
    fn memory_check_for(bytes: u64) -> Check {
        let answer = format!("{bytes}\n");
        let runner = FakeRunner::answering_for(&[("sysctl", answer.as_str())]);
        find(&preflight(&runner, Path::new("."), &[]), "memory").clone()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn memory_at_36_gib_is_ok_with_the_whole_gib_in_the_detail() {
        let check = memory_check_for(38654705664);
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.detail, "36 GiB");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn memory_thresholds_compare_the_exact_value() {
        assert_eq!(memory_check_for(8 * GIB).status, Status::Ok);
        assert_eq!(memory_check_for(8 * GIB - 1).status, Status::Warn);
        assert_eq!(memory_check_for(6 * GIB).status, Status::Warn);
        assert_eq!(memory_check_for(6 * GIB - 1).status, Status::Fail);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn memory_is_not_checked_when_sysctl_gives_no_number() {
        let runner = FakeRunner::answering_for(&[("sysctl", "unknown\n")]);
        assert_eq!(memory_gib(&runner), None);
        let check = find(&preflight(&runner, Path::new("."), &[]), "memory").clone();
        assert_eq!(check.status, Status::Warn);
        assert_eq!(check.detail, "memory not checked");
    }

    #[test]
    fn meminfo_text_gives_the_total_in_gib() {
        let text = "MemFree:         1000000 kB\nMemTotal:       16384000 kB\nMemAvailable:    900000 kB\n";
        assert_eq!(parse_meminfo(text), Some(16384000.0 / 1024.0 / 1024.0));
        assert_eq!(parse_meminfo("MemFree: 1 kB\n"), None);
        assert_eq!(parse_meminfo("MemTotal: lots kB\n"), None);
    }

    #[test]
    fn disk_compares_the_free_column_with_five_gib() {
        let at = |free_kib: u64| {
            let answer = disk_answer(free_kib);
            let runner = FakeRunner::answering_for(&[("df", answer.as_str())]);
            find(&preflight(&runner, Path::new("."), &[]), "disk").clone()
        };
        let low = at(4 * 1024 * 1024);
        assert_eq!(low.status, Status::Fail);
        assert_eq!(low.detail, "4 GiB free");
        let enough = at(5 * 1024 * 1024);
        assert_eq!(enough.status, Status::Ok);
        assert_eq!(enough.detail, "5 GiB free");
    }

    #[test]
    fn disk_asks_df_for_the_folder_and_is_not_checked_on_bad_output() {
        let runner = FakeRunner::answering_for(&[("df", "Filesystem only\n")]);
        assert_eq!(disk_free_gib(&runner, Path::new("some/dir")), None);
        let calls = runner.outputs();
        assert_eq!(calls[0].program, "df");
        assert_eq!(calls[0].args, ["-k", "some/dir"]);
        assert_eq!(
            disk_free_gib(&FakeRunner::returning(1), Path::new(".")),
            None
        );
        let check = find(
            &preflight(&FakeRunner::failing(), Path::new("."), &[]),
            "disk",
        )
        .clone();
        assert_eq!(check.status, Status::Warn);
        assert_eq!(check.detail, "disk not checked");
    }

    #[test]
    fn docker_and_compose_pass_with_the_first_line_and_fail_otherwise() {
        let runner = FakeRunner::answering("Docker version 27.0.1\nmore\n");
        let checks = preflight(&runner, Path::new("."), &[]);
        let docker = find(&checks, "docker");
        assert_eq!(docker.status, Status::Ok);
        assert_eq!(docker.detail, "Docker version 27.0.1");
        assert_eq!(find(&checks, "compose").status, Status::Ok);
        let calls = runner.outputs();
        assert_eq!(calls[0].args, ["--version"]);
        assert_eq!(calls[1].args, ["compose", "version"]);

        let checks = preflight(&FakeRunner::returning(3), Path::new("."), &[]);
        let compose = find(&checks, "compose");
        assert_eq!(compose.status, Status::Fail);
        assert_eq!(compose.detail, "exited with code 3");
    }

    #[test]
    fn a_docker_that_cannot_start_fails_with_the_error_text() {
        let checks = preflight(&FakeRunner::failing(), Path::new("."), &[]);
        let docker = find(&checks, "docker");
        assert_eq!(docker.status, Status::Fail);
        assert!(docker.detail.contains("fake runner failure"), "{docker:?}");
    }

    #[test]
    fn an_unreachable_host_only_warns_and_the_five_checks_keep_their_order() {
        let host = closed_host();
        let checks = preflight(&FakeRunner::returning(0), Path::new("."), &[&host]);
        let names: Vec<&str> = checks.iter().map(|check| check.name.as_str()).collect();
        assert_eq!(names, ["docker", "compose", "memory", "disk", "network"]);
        let network = find(&checks, "network");
        assert_eq!(network.status, Status::Warn);
        assert_eq!(network.detail, format!("no answer from {host}"));
    }

    #[test]
    fn network_counts_the_hosts_when_all_answer_and_lists_the_silent_ones_in_order() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let open = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let runner = FakeRunner::returning(0);
        let checks = preflight(&runner, Path::new("."), &[&open, &open]);
        let network = find(&checks, "network");
        assert_eq!(network.status, Status::Ok);
        assert_eq!(network.detail, "2 hosts answered");

        let (first, second) = (closed_host(), closed_host());
        let checks = preflight(&runner, Path::new("."), &[&first, &open, &second]);
        assert_eq!(
            find(&checks, "network").detail,
            format!("no answer from {first}, {second}")
        );
    }

    #[test]
    fn a_host_with_a_listener_is_reachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        assert!(reachable(&host));
    }
}
