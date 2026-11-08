use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;

pub const PROJECT: &str = "netobs";
pub const NETWORK: &str = "network-observability";
pub const SUBNET: &str = "198.51.100.0/24";
pub const CEOS_KINDS: [&str; 2] = ["ceos", "arista_ceos"];

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
    nodes: BTreeMap<String, Option<Settings>>,
    defaults: Option<Settings>,
    kinds: Option<BTreeMap<String, Option<Settings>>>,
}

#[derive(Deserialize)]
struct Settings {
    kind: Option<String>,
    image: Option<String>,
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

#[derive(Debug, PartialEq)]
pub struct NodeImage {
    pub nodes: Vec<String>,
    pub kind: String,
    pub image: String,
    pub variable: Option<String>,
}

enum ExpandError {
    Unset(String),
    Unsupported(String),
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn expand_value(
    value: &str,
    variable: &impl Fn(&str) -> Option<String>,
) -> Result<(String, Option<String>), ExpandError> {
    let mut out = String::new();
    let mut first = None;
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if let Some(after) = rest.strip_prefix("$$") {
            out.push('$');
            rest = after;
            continue;
        }
        let Some(body) = rest.strip_prefix("${") else {
            let end = rest[1..]
                .find(|c: char| !is_name_char(c))
                .map_or(rest.len(), |n| n + 1);
            return Err(ExpandError::Unsupported(rest[..end].to_string()));
        };
        let Some(close) = body.find('}') else {
            return Err(ExpandError::Unsupported(rest.to_string()));
        };
        let text = &rest[..close + 3];
        let inner = &body[..close];
        let name_end = inner
            .find(|c: char| !is_name_char(c))
            .unwrap_or(inner.len());
        let (name, tail) = inner.split_at(name_end);
        if !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            return Err(ExpandError::Unsupported(text.to_string()));
        }
        let (default, empty_is_unset) = if tail.is_empty() {
            (None, false)
        } else if let Some(default) = tail.strip_prefix(":-").or_else(|| tail.strip_prefix(":=")) {
            (Some(default), true)
        } else if let Some(default) = tail.strip_prefix('-').or_else(|| tail.strip_prefix('=')) {
            (Some(default), false)
        } else {
            return Err(ExpandError::Unsupported(text.to_string()));
        };
        if default.is_some_and(|default| default.contains('$')) {
            return Err(ExpandError::Unsupported(text.to_string()));
        }
        first.get_or_insert_with(|| name.to_string());
        match (variable(name), default) {
            (Some(found), _) if !(empty_is_unset && found.is_empty()) => out.push_str(&found),
            (_, Some(default)) => out.push_str(default),
            (_, None) => return Err(ExpandError::Unset(name.to_string())),
        }
        rest = &rest[close + 3..];
    }
    out.push_str(rest);
    Ok((out, first))
}

pub fn expand(value: &str, variable: impl Fn(&str) -> Option<String>) -> Result<String, String> {
    expand_value(value, &variable)
        .map(|(text, _)| text)
        .map_err(|error| match error {
            ExpandError::Unset(name) => format!("variable {name} is not set"),
            ExpandError::Unsupported(text) => format!("unsupported variable form: {text}"),
        })
}

pub fn node_images(
    topology_text: &str,
    variable: impl Fn(&str) -> Option<String>,
) -> Result<Vec<NodeImage>, String> {
    let topology: Topology =
        serde_saphyr::from_str(topology_text).map_err(|error| error.to_string())?;
    let section = &topology.topology;
    let mut images: BTreeMap<String, NodeImage> = BTreeMap::new();
    for (node, settings) in &section.nodes {
        let kind = settings
            .as_ref()
            .and_then(|own| own.kind.as_deref())
            .or_else(|| section.defaults.as_ref().and_then(|d| d.kind.as_deref()))
            .unwrap_or_default();
        let Some(raw) = settings
            .as_ref()
            .and_then(|own| own.image.as_deref())
            .or_else(|| {
                section
                    .kinds
                    .as_ref()
                    .and_then(|kinds| kinds.get(kind))
                    .and_then(Option::as_ref)
                    .and_then(|of_kind| of_kind.image.as_deref())
            })
            .or_else(|| section.defaults.as_ref().and_then(|d| d.image.as_deref()))
        else {
            continue;
        };
        let (image, name) = expand_value(raw, &variable).map_err(|error| match error {
            ExpandError::Unset(name) => {
                format!("variable {name} is not set (image of node {node})")
            }
            ExpandError::Unsupported(text) => {
                format!("unsupported variable form in the image of node {node}: {text}")
            }
        })?;
        images
            .entry(image.clone())
            .or_insert_with(|| NodeImage {
                nodes: Vec::new(),
                kind: kind.to_string(),
                image,
                variable: name,
            })
            .nodes
            .push(node.clone());
    }
    Ok(images.into_values().collect())
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
    pub port: Option<String>,
    pub age: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Publisher {
    published_port: Option<u16>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ServiceRow {
    service: String,
    state: String,
    health: Option<String>,
    publishers: Option<Vec<Publisher>>,
    running_for: Option<String>,
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
            port: row
                .publishers
                .unwrap_or_default()
                .iter()
                .find_map(|publisher| publisher.published_port.filter(|port| *port != 0))
                .map(|port| port.to_string()),
            age: row
                .running_for
                .map(|text| text.strip_suffix(" ago").unwrap_or(&text).to_string()),
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
            port: None,
            age: None,
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
        let rastreo =
            "name: rastreo-lab\ntopology:\n  nodes:\n    srl-01:\n      kind: nokia_srlinux\n";
        assert_eq!(
            node_containers(rastreo),
            Ok(strings(&["clab-rastreo-lab-srl-01"]))
        );
        assert_eq!(with("prefix: \"\"\n"), Ok(strings(&["a", "b"])));
        assert_eq!(
            with("prefix: __lab-name\n"),
            Ok(strings(&["lab-a", "lab-b"]))
        );
        assert_eq!(with("prefix: x\n"), Ok(strings(&["x-lab-a", "x-lab-b"])));
    }

    #[test]
    fn node_containers_reject_text_that_is_not_a_topology() {
        assert!(node_containers("a: [b").is_err());
        assert!(node_containers("name: lab\n").is_err());
        assert!(node_containers("topology:\n  nodes:\n    a: {}\n").is_err());
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn expand_gives_the_value_of_a_set_variable() {
        assert_eq!(expand("${A}", env(&[("A", "x")])), Ok("x".to_string()));
        assert_eq!(expand("${A}", env(&[("A", "")])), Ok(String::new()));
    }

    #[test]
    fn expand_colon_dash_and_colon_equals_give_the_default_for_an_unset_or_empty_variable() {
        for form in ["${A:-d}", "${A:=d}"] {
            assert_eq!(expand(form, env(&[])), Ok("d".to_string()), "{form}");
            assert_eq!(
                expand(form, env(&[("A", "")])),
                Ok("d".to_string()),
                "{form}"
            );
            assert_eq!(
                expand(form, env(&[("A", "x")])),
                Ok("x".to_string()),
                "{form}"
            );
        }
    }

    #[test]
    fn expand_dash_and_equals_give_the_default_only_for_an_unset_variable() {
        for form in ["${A-d}", "${A=d}"] {
            assert_eq!(expand(form, env(&[])), Ok("d".to_string()), "{form}");
            assert_eq!(expand(form, env(&[("A", "")])), Ok(String::new()), "{form}");
            assert_eq!(
                expand(form, env(&[("A", "x")])),
                Ok("x".to_string()),
                "{form}"
            );
        }
    }

    #[test]
    fn expand_turns_two_dollars_into_one() {
        assert_eq!(expand("a$$b$$", env(&[])), Ok("a$b$".to_string()));
        assert_eq!(expand("$${A}", env(&[])), Ok("${A}".to_string()));
    }

    #[test]
    fn expand_of_an_unset_variable_without_a_default_names_the_variable() {
        assert_eq!(
            expand("x${A}", env(&[])),
            Err("variable A is not set".to_string())
        );
    }

    #[test]
    fn expand_refuses_every_other_dollar_form_and_names_it() {
        for (value, form) in [
            ("${A:+x}", "${A:+x}"),
            ("${A+x}", "${A+x}"),
            ("${A:?x}", "${A:?x}"),
            ("$A", "$A"),
            ("a $A b", "$A"),
            ("a$", "$"),
            ("${A", "${A"),
            ("${1A}", "${1A}"),
            ("${}", "${}"),
            ("${A:-${B}}", "${A:-${B}"),
        ] {
            assert_eq!(
                expand(value, env(&[("A", "x")])),
                Err(format!("unsupported variable form: {form}")),
                "{value}"
            );
        }
    }

    #[test]
    fn expand_expands_every_form_in_a_value_and_copies_the_rest() {
        assert_eq!(
            expand("r/${A}:${B:-b}-$$-end", env(&[("A", "x")])),
            Ok("r/x:b-$-end".to_string())
        );
    }

    fn book_lab(image: &str) -> String {
        format!(
            "name: book\ntopology:\n  kinds:\n    ceos:\n      image: {image}\n  nodes:\n    ceos-01:\n      kind: ceos\n    ceos-02:\n      kind: ceos\n"
        )
    }

    #[test]
    fn node_images_of_the_fixture_is_one_image_with_both_nodes() {
        let fixture = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/fixtures/book-lab/containerlab/lab.yml"),
        )
        .unwrap();
        assert_eq!(
            node_images(&fixture, env(&[])),
            Ok(vec![NodeImage {
                nodes: strings(&["nobs-t-srl1", "nobs-t-srl2"]),
                kind: "nokia_srlinux".to_string(),
                image: "ghcr.io/nokia/srlinux:latest".to_string(),
                variable: None,
            }])
        );
    }

    #[test]
    fn node_images_of_the_book_lab_uses_the_default_and_names_the_variable() {
        assert_eq!(
            node_images(&book_lab("${CEOS_IMAGE:-ceos:image}"), env(&[])),
            Ok(vec![NodeImage {
                nodes: strings(&["ceos-01", "ceos-02"]),
                kind: "ceos".to_string(),
                image: "ceos:image".to_string(),
                variable: Some("CEOS_IMAGE".to_string()),
            }])
        );
    }

    #[test]
    fn node_images_of_the_book_lab_uses_the_value_of_the_variable() {
        assert_eq!(
            node_images(
                &book_lab("${CEOS_IMAGE:-ceos:image}"),
                env(&[("CEOS_IMAGE", "ceos:4.32.0F")])
            ),
            Ok(vec![NodeImage {
                nodes: strings(&["ceos-01", "ceos-02"]),
                kind: "ceos".to_string(),
                image: "ceos:4.32.0F".to_string(),
                variable: Some("CEOS_IMAGE".to_string()),
            }])
        );
    }

    #[test]
    fn node_images_prefers_the_image_of_the_node_over_the_image_of_its_kind() {
        let text = "name: t\ntopology:\n  kinds:\n    ceos:\n      image: kind:1\n  nodes:\n    a:\n      kind: ceos\n      image: own:1\n    b:\n      kind: ceos\n";
        assert_eq!(
            node_images(text, env(&[])),
            Ok(vec![
                NodeImage {
                    nodes: strings(&["b"]),
                    kind: "ceos".to_string(),
                    image: "kind:1".to_string(),
                    variable: None,
                },
                NodeImage {
                    nodes: strings(&["a"]),
                    kind: "ceos".to_string(),
                    image: "own:1".to_string(),
                    variable: None,
                },
            ])
        );
    }

    #[test]
    fn node_images_uses_the_image_and_kind_of_defaults_last() {
        let text = "name: t\ntopology:\n  defaults:\n    kind: ceos\n    image: default:1\n  kinds:\n    ceos:\n      image: kind:1\n    linux:\n  nodes:\n    a:\n      kind: linux\n    b: {}\n    c:\n";
        assert_eq!(
            node_images(text, env(&[])),
            Ok(vec![
                NodeImage {
                    nodes: strings(&["a"]),
                    kind: "linux".to_string(),
                    image: "default:1".to_string(),
                    variable: None,
                },
                NodeImage {
                    nodes: strings(&["b", "c"]),
                    kind: "ceos".to_string(),
                    image: "kind:1".to_string(),
                    variable: None,
                },
            ])
        );
    }

    #[test]
    fn node_images_groups_nodes_by_expanded_image_and_takes_the_first_node_for_kind_and_variable() {
        let text = "name: t\ntopology:\n  nodes:\n    c:\n      kind: k3\n      image: i:1\n    b:\n      kind: k2\n      image: ${V:-i:1}\n    a:\n      kind: k1\n      image: h:1\n";
        assert_eq!(
            node_images(text, env(&[])),
            Ok(vec![
                NodeImage {
                    nodes: strings(&["a"]),
                    kind: "k1".to_string(),
                    image: "h:1".to_string(),
                    variable: None,
                },
                NodeImage {
                    nodes: strings(&["b", "c"]),
                    kind: "k2".to_string(),
                    image: "i:1".to_string(),
                    variable: Some("V".to_string()),
                },
            ])
        );
    }

    #[test]
    fn node_images_does_not_count_two_dollars_as_a_variable() {
        let text = "name: t\ntopology:\n  nodes:\n    a:\n      image: i$$1\n";
        let images = node_images(text, env(&[])).unwrap();
        assert_eq!(images[0].image, "i$1");
        assert_eq!(images[0].variable, None);
    }

    #[test]
    fn node_images_skips_a_node_that_has_no_image_from_any_source() {
        let text = "name: t\ntopology:\n  kinds:\n    ceos:\n      image: kind:1\n  nodes:\n    a:\n      kind: ceos\n    b:\n      kind: linux\n";
        assert_eq!(
            node_images(text, env(&[])),
            Ok(vec![NodeImage {
                nodes: strings(&["a"]),
                kind: "ceos".to_string(),
                image: "kind:1".to_string(),
                variable: None,
            }])
        );
    }

    #[test]
    fn node_images_names_the_node_when_the_image_cannot_be_expanded() {
        let unset = "name: t\ntopology:\n  nodes:\n    a:\n      image: ${V}\n";
        assert_eq!(
            node_images(unset, env(&[])),
            Err("variable V is not set (image of node a)".to_string())
        );
        let unsupported = "name: t\ntopology:\n  nodes:\n    a:\n      image: $V\n";
        assert_eq!(
            node_images(unsupported, env(&[])),
            Err("unsupported variable form in the image of node a: $V".to_string())
        );
    }

    #[test]
    fn node_images_rejects_text_that_is_not_a_topology() {
        assert!(node_images("a: [b", env(&[])).is_err());
        assert!(node_images("name: lab\n", env(&[])).is_err());
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
    fn services_reads_the_published_port_and_the_age_without_ago() {
        let text = "{\"Service\":\"a\",\"State\":\"running\",\"Publishers\":[{\"URL\":\"0.0.0.0\",\"TargetPort\":3000,\"PublishedPort\":3000,\"Protocol\":\"tcp\"}],\"RunningFor\":\"41 seconds ago\"}\n";
        let found = services(text).unwrap();
        assert_eq!(found[0].port, Some("3000".to_string()));
        assert_eq!(found[0].age, Some("41 seconds".to_string()));
    }

    #[test]
    fn services_takes_the_first_publisher_with_a_port_that_is_not_zero() {
        let text = "{\"Service\":\"a\",\"State\":\"running\",\"Publishers\":[{\"TargetPort\":80,\"PublishedPort\":0},{\"TargetPort\":9090,\"PublishedPort\":9090}]}";
        assert_eq!(services(text).unwrap()[0].port, Some("9090".to_string()));
    }

    #[test]
    fn services_without_publishers_and_running_for_has_no_port_and_no_age() {
        let found = services(LINES).unwrap();
        assert_eq!(found[1].port, None);
        assert_eq!(found[1].age, None);
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
