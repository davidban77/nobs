use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::de::IgnoredAny;
use serde::Deserialize;

pub const PROJECT: &str = "netobs";
pub const NETWORK: &str = "network-observability";
pub const SUBNET: &str = "198.51.100.0/24";

const POLL: Duration = Duration::from_secs(5);

pub fn scenarios(dir: &Path) -> Result<Vec<String>, String> {
    let chapters = dir.join("chapters");
    let entries =
        fs::read_dir(&chapters).map_err(|error| format!("{}: {error}", chapters.display()))?;
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().join("docker-compose.yml").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

#[derive(Deserialize)]
struct Topology {
    name: String,
    prefix: Option<String>,
    topology: Nodes,
}

#[derive(Deserialize)]
struct Nodes {
    nodes: BTreeMap<String, IgnoredAny>,
}

pub fn node_containers(topology_text: &str) -> Result<Vec<String>, String> {
    let topology: Topology =
        serde_saphyr::from_str(topology_text).map_err(|error| error.to_string())?;
    let lab = &topology.name;
    let mut names: Vec<String> = topology
        .topology
        .nodes
        .keys()
        .map(|node| match topology.prefix.as_deref() {
            None => format!("clab-{lab}-{node}"),
            Some("") => node.clone(),
            Some("__lab-name") => format!("{lab}-{node}"),
            Some(prefix) => format!("{prefix}-{lab}-{node}"),
        })
        .collect();
    names.sort();
    Ok(names)
}

pub fn truth(value: &str) -> Result<bool, String> {
    match value.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" | "" => Ok(false),
        _ => Err(format!("{value:?} is not true or false")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub state: String,
    pub health: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ServiceRow {
    service: String,
    state: String,
    health: Option<String>,
}

pub fn services(ps_json: &str) -> Result<Vec<Service>, String> {
    let text = ps_json.trim();
    let rows: Vec<ServiceRow> = if text.starts_with('[') {
        serde_json::from_str(text).map_err(|error| error.to_string())?
    } else {
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .map_err(|error| error.to_string())?
    };
    Ok(rows
        .into_iter()
        .map(|row| Service {
            name: row.service,
            state: row.state,
            health: row.health.unwrap_or_default(),
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub state: String,
}

#[derive(Deserialize)]
struct NodeRow {
    name: String,
    state: String,
}

pub fn nodes(inspect_json: &str) -> Result<Vec<Node>, String> {
    if inspect_json.trim().is_empty() {
        return Ok(Vec::new());
    }
    let labs: BTreeMap<String, Vec<NodeRow>> =
        serde_json::from_str(inspect_json).map_err(|error| error.to_string())?;
    Ok(labs
        .into_values()
        .flatten()
        .map(|row| Node {
            name: row.name,
            state: row.state,
        })
        .collect())
}

pub fn not_ready(services: &[Service], nodes: &[Node], expected_nodes: &[String]) -> Vec<String> {
    let mut waiting = Vec::new();
    if services.is_empty() {
        waiting.push("no services listed".to_string());
    }
    for service in services {
        let healthy = service.health.is_empty() || service.health == "healthy";
        if service.state != "running" || !healthy {
            waiting.push(format!(
                "{} ({}/{})",
                service.name, service.state, service.health
            ));
        }
    }
    for expected in expected_nodes {
        match nodes.iter().find(|node| &node.name == expected) {
            Some(node) if node.state == "running" => {}
            Some(node) => waiting.push(format!("{} ({})", node.name, node.state)),
            None => waiting.push(format!("{expected} (missing)")),
        }
    }
    waiting
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitError<E> {
    NotReady(Vec<String>),
    Failed(E),
}

pub fn wait_ready<E>(
    mut poll: impl FnMut() -> Result<Vec<String>, E>,
    limit: Duration,
    mut sleep: impl FnMut(Duration),
    mut now: impl FnMut() -> Instant,
) -> Result<Duration, WaitError<E>> {
    let start = now();
    loop {
        let waiting = poll().map_err(WaitError::Failed)?;
        let elapsed = now().duration_since(start);
        if waiting.is_empty() {
            return Ok(elapsed);
        }
        if elapsed >= limit {
            return Err(WaitError::NotReady(waiting));
        }
        sleep((limit - elapsed).min(POLL));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("nobs-lab-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn file(&self, relative: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|text| text.to_string()).collect()
    }

    fn service(name: &str, state: &str, health: &str) -> Service {
        Service {
            name: name.to_string(),
            state: state.to_string(),
            health: health.to_string(),
        }
    }

    fn node(name: &str, state: &str) -> Node {
        Node {
            name: name.to_string(),
            state: state.to_string(),
        }
    }

    #[test]
    fn scenarios_are_the_sorted_chapter_folders_that_hold_a_compose_file() {
        let scratch = Scratch::new("scenarios");
        scratch.file("chapters/ch2/docker-compose.yml");
        scratch.file("chapters/ch1/docker-compose.yml");
        scratch.file("chapters/notes/readme.md");
        scratch.file("chapters/loose.yml");
        assert_eq!(scenarios(&scratch.0), Ok(strings(&["ch1", "ch2"])));
    }

    #[test]
    fn scenarios_without_a_chapters_folder_is_an_error_naming_the_path() {
        let scratch = Scratch::new("no-chapters");
        let error = scenarios(&scratch.0).unwrap_err();
        assert!(
            error.starts_with(&scratch.0.join("chapters").display().to_string()),
            "{error}"
        );
    }

    #[test]
    fn node_containers_follow_the_containerlab_naming_rule() {
        let fixture = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/fixtures/book-lab/containerlab/lab.yml"),
        )
        .unwrap();
        assert_eq!(
            node_containers(&fixture),
            Ok(strings(&[
                "clab-nobs-t-book-nobs-t-srl1",
                "clab-nobs-t-book-nobs-t-srl2"
            ]))
        );
        let with = |prefix: &str| {
            node_containers(&format!(
                "name: lab\n{prefix}topology:\n  nodes:\n    b: {{}}\n    a: {{}}\n"
            ))
        };
        assert_eq!(with(""), Ok(strings(&["clab-lab-a", "clab-lab-b"])));
        assert_eq!(with("prefix: \"\"\n"), Ok(strings(&["a", "b"])));
        assert_eq!(
            with("prefix: __lab-name\n"),
            Ok(strings(&["lab-a", "lab-b"]))
        );
        assert_eq!(with("prefix: x\n"), Ok(strings(&["x-lab-a", "x-lab-b"])));
    }

    #[test]
    fn node_containers_of_a_lab_without_a_prefix_key_keep_the_clab_prefix() {
        let text =
            "name: rastreo-lab\ntopology:\n  nodes:\n    srl-01:\n      kind: nokia_srlinux\n";
        assert_eq!(
            node_containers(text),
            Ok(strings(&["clab-rastreo-lab-srl-01"]))
        );
    }

    #[test]
    fn node_containers_reject_text_that_is_not_a_topology() {
        assert!(node_containers("a: [b").is_err());
        assert!(node_containers("name: lab\n").is_err());
        assert!(node_containers("topology:\n  nodes:\n    a: {}\n").is_err());
    }

    #[test]
    fn truth_reads_the_eight_words_in_any_case_and_the_empty_text() {
        for word in ["1", "true", "yes", "on", "TRUE", "Yes", "ON", " true "] {
            assert_eq!(truth(word), Ok(true), "{word}");
        }
        for word in ["0", "false", "no", "off", "FALSE", "No", "OFF", "", "  "] {
            assert_eq!(truth(word), Ok(false), "{word}");
        }
        assert!(truth("maybe").unwrap_err().contains("maybe"));
    }

    const LINES: &str = "{\"Service\":\"a\",\"State\":\"running\",\"Health\":\"healthy\"}\n{\"Service\":\"b\",\"State\":\"exited\"}\n";
    const LIST: &str = "[{\"Service\":\"a\",\"State\":\"running\",\"Health\":\"healthy\"},{\"Service\":\"b\",\"State\":\"exited\",\"Health\":\"\"}]";

    #[test]
    fn services_reads_one_object_per_line_and_one_list() {
        let expected = vec![
            service("a", "running", "healthy"),
            service("b", "exited", ""),
        ];
        assert_eq!(services(LINES), Ok(expected.clone()));
        assert_eq!(services(LIST), Ok(expected));
    }

    #[test]
    fn services_of_blank_text_is_empty_and_of_other_text_an_error() {
        assert_eq!(services(""), Ok(vec![]));
        assert_eq!(services(" \n "), Ok(vec![]));
        assert!(services("not json").is_err());
        assert!(services("[1").is_err());
    }

    #[test]
    fn nodes_takes_every_node_of_every_lab() {
        let text = "{\"rastreo-lab\":[{\"name\":\"clab-rastreo-lab-srl-01\",\"state\":\"running\",\"image\":\"x\"},{\"name\":\"clab-rastreo-lab-srl-02\",\"state\":\"exited\"}],\"other\":[{\"name\":\"clab-other-a\",\"state\":\"running\"}]}";
        assert_eq!(
            nodes(text),
            Ok(vec![
                node("clab-other-a", "running"),
                node("clab-rastreo-lab-srl-01", "running"),
                node("clab-rastreo-lab-srl-02", "exited"),
            ])
        );
    }

    #[test]
    fn nodes_of_blank_text_or_an_empty_object_is_empty_and_of_other_text_an_error() {
        assert_eq!(nodes(""), Ok(vec![]));
        assert_eq!(nodes("\n"), Ok(vec![]));
        assert_eq!(nodes("{}"), Ok(vec![]));
        assert!(nodes("not json").is_err());
    }

    #[test]
    fn a_running_service_without_or_with_a_healthy_check_is_ready() {
        let services = [
            service("a", "running", ""),
            service("b", "running", "healthy"),
        ];
        assert_eq!(not_ready(&services, &[], &[]), Vec::<String>::new());
    }

    #[test]
    fn exited_unhealthy_and_starting_services_are_not_ready() {
        let services = [
            service("a", "exited", ""),
            service("b", "running", "unhealthy"),
            service("c", "running", "starting"),
            service("d", "created", ""),
        ];
        assert_eq!(
            not_ready(&services, &[], &[]),
            [
                "a (exited/)",
                "b (running/unhealthy)",
                "c (running/starting)",
                "d (created/)"
            ]
        );
    }

    #[test]
    fn expected_nodes_must_be_running_and_other_nodes_do_not_count() {
        let services = [service("a", "running", "")];
        let nodes = [
            node("n1", "running"),
            node("n2", "exited"),
            node("other-lab-node", "exited"),
        ];
        assert_eq!(
            not_ready(&services, &nodes, &strings(&["n1", "n2", "n3"])),
            ["n2 (exited)", "n3 (missing)"]
        );
    }

    #[test]
    fn no_services_listed_is_not_ready() {
        assert_eq!(not_ready(&[], &[], &[]), ["no services listed"]);
    }

    struct Clock {
        start: Instant,
        elapsed: Cell<Duration>,
    }

    impl Clock {
        fn new() -> Self {
            Self {
                start: Instant::now(),
                elapsed: Cell::new(Duration::ZERO),
            }
        }

        fn now(&self) -> Instant {
            self.start + self.elapsed.get()
        }

        fn advance(&self, by: Duration) {
            self.elapsed.set(self.elapsed.get() + by);
        }
    }

    fn seconds(count: u64) -> Duration {
        Duration::from_secs(count)
    }

    fn waiting(items: &[&str]) -> Result<Vec<String>, ()> {
        Ok(strings(items))
    }

    #[test]
    fn wait_ready_polls_at_once_and_does_not_sleep_when_ready() {
        let clock = Clock::new();
        let sleeps = RefCell::new(Vec::new());
        let polls = Cell::new(0);
        let result = wait_ready(
            || {
                polls.set(polls.get() + 1);
                waiting(&[])
            },
            seconds(60),
            |by| sleeps.borrow_mut().push(by),
            || clock.now(),
        );
        assert_eq!(result, Ok(Duration::ZERO));
        assert_eq!(polls.get(), 1);
        assert!(sleeps.borrow().is_empty());
    }

    #[test]
    fn wait_ready_sleeps_five_seconds_between_polls_and_reports_the_elapsed_time() {
        let clock = Clock::new();
        let sleeps = RefCell::new(Vec::new());
        let mut answers = [vec!["a"], vec!["a"], vec![]].into_iter();
        let result = wait_ready(
            || waiting(&answers.next().unwrap()),
            seconds(60),
            |by| {
                sleeps.borrow_mut().push(by);
                clock.advance(by);
            },
            || clock.now(),
        );
        assert_eq!(result, Ok(seconds(10)));
        assert_eq!(*sleeps.borrow(), [seconds(5), seconds(5)]);
    }

    #[test]
    fn wait_ready_gives_the_last_list_when_the_limit_is_reached_and_cuts_the_last_sleep() {
        let clock = Clock::new();
        let sleeps = RefCell::new(Vec::new());
        let polls = Cell::new(0);
        let result = wait_ready(
            || {
                polls.set(polls.get() + 1);
                Ok::<_, ()>(vec![format!("poll {}", polls.get())])
            },
            seconds(7),
            |by| {
                sleeps.borrow_mut().push(by);
                clock.advance(by);
            },
            || clock.now(),
        );
        assert_eq!(result, Err(WaitError::NotReady(strings(&["poll 3"]))));
        assert_eq!(*sleeps.borrow(), [seconds(5), seconds(2)]);
        assert_eq!(polls.get(), 3);
    }

    #[test]
    fn wait_ready_stops_at_once_on_a_poll_error() {
        let clock = Clock::new();
        let sleeps = RefCell::new(Vec::new());
        let polls = Cell::new(0);
        let result = wait_ready(
            || {
                polls.set(polls.get() + 1);
                if polls.get() == 2 {
                    Err("boom")
                } else {
                    Ok(strings(&["a"]))
                }
            },
            seconds(60),
            |by| {
                sleeps.borrow_mut().push(by);
                clock.advance(by);
            },
            || clock.now(),
        );
        assert_eq!(result, Err(WaitError::Failed("boom")));
        assert_eq!(polls.get(), 2);
        assert_eq!(sleeps.borrow().len(), 1);
    }
}
