use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::marker::PhantomData;
use std::path::Path;

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, Serializer};
use serde::Deserialize;
use serde_saphyr::Spanned;

use crate::render::{NodeVars, Renderer, TEMPLATES};

pub const FILE: &str = "nobs_project.yml";

pub const TOPOLOGY: &str = "containerlab/lab.clab.yml";

#[derive(Debug, Clone, PartialEq)]
enum Ordered {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    Text(String),
    List(Vec<Ordered>),
    Map(Vec<(String, Ordered)>),
}

impl Ordered {
    fn get(&self, key: &str) -> Option<&Ordered> {
        match self {
            Ordered::Map(entries) => entries.iter().find(|(name, _)| name == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn to_json(&self) -> serde_json::Value {
        use serde_json::Value;
        match self {
            Ordered::Null => Value::Null,
            Ordered::Bool(value) => Value::Bool(*value),
            Ordered::Integer(value) => Value::from(*value),
            Ordered::Float(value) => Value::from(*value),
            Ordered::Text(value) => Value::String(value.clone()),
            Ordered::List(items) => Value::Array(items.iter().map(Ordered::to_json).collect()),
            Ordered::Map(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
            ),
        }
    }
}

fn text(value: &str) -> Ordered {
    Ordered::Text(value.to_string())
}

fn map(entries: Vec<(&str, Ordered)>) -> Ordered {
    Ordered::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

fn entries<'de, A, K, V>(mut map: A) -> Result<Vec<(K, V)>, A::Error>
where
    A: MapAccess<'de>,
    K: Deserialize<'de>,
    V: Deserialize<'de>,
{
    let mut list = Vec::new();
    while let Some(entry) = map.next_entry()? {
        list.push(entry);
    }
    Ok(list)
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Any;

        impl<'de> Visitor<'de> for Any {
            type Value = Ordered;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a YAML value")
            }

            fn visit_unit<E>(self) -> Result<Ordered, E> {
                Ok(Ordered::Null)
            }

            fn visit_bool<E>(self, value: bool) -> Result<Ordered, E> {
                Ok(Ordered::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Ordered, E> {
                Ok(Ordered::Integer(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Ordered, E> {
                Ok(i64::try_from(value).map_or(Ordered::Float(value as f64), Ordered::Integer))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Ordered, E> {
                Ok(Ordered::Float(value))
            }

            fn visit_str<E>(self, value: &str) -> Result<Ordered, E> {
                Ok(text(value))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Ordered, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Ordered::List(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Ordered, A::Error> {
                Ok(Ordered::Map(entries(map)?))
            }
        }

        deserializer.deserialize_any(Any)
    }
}

impl Serialize for Ordered {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Ordered::Null => serializer.serialize_unit(),
            Ordered::Bool(value) => serializer.serialize_bool(*value),
            Ordered::Integer(value) => serializer.serialize_i64(*value),
            Ordered::Float(value) => serializer.serialize_f64(*value),
            Ordered::Text(value) => serializer.serialize_str(value),
            Ordered::List(items) => serializer.collect_seq(items),
            Ordered::Map(entries) => {
                serializer.collect_map(entries.iter().map(|(key, value)| (key, value)))
            }
        }
    }
}

fn in_order<'de, D, K, V>(deserializer: D) -> Result<Vec<(K, V)>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de>,
    V: Deserialize<'de>,
{
    struct Entries<K, V>(PhantomData<(K, V)>);

    impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> Visitor<'de> for Entries<K, V> {
        type Value = Vec<(K, V)>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a map")
        }

        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            entries(map)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_any(Entries(PhantomData))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    main: Main,
    nodes: Vec<Spanned<RawGroup>>,
    #[serde(default, deserialize_with = "in_order")]
    links: Vec<(Spanned<String>, Vec<Spanned<RawLink>>)>,
    vars: Ordered,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Main {
    name: String,
    #[serde(rename = "description")]
    _description: Option<String>,
    mgmt: Mgmt,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mgmt {
    network: String,
    subnet: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGroup {
    kind: Option<String>,
    image: String,
    #[serde(default, deserialize_with = "in_order")]
    labels: Vec<(String, String)>,
    nodes: Vec<Spanned<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLink {
    port: String,
    node_b: String,
    port_b: String,
}

#[derive(Debug)]
struct Group {
    kind: String,
    image: String,
    labels: Vec<(String, String)>,
    nodes: Vec<String>,
}

#[derive(Debug)]
struct Link {
    node: String,
    port: String,
    node_b: String,
    port_b: String,
}

#[derive(Debug)]
pub struct Project {
    main: Main,
    groups: Vec<Group>,
    links: Vec<Link>,
    vars: Ordered,
}

pub fn read(text: &str) -> Result<Project, String> {
    let file: File = serde_saphyr::from_str(text).map_err(|error| error.to_string())?;
    let mut known = HashSet::new();
    let mut groups = Vec::new();
    for spanned in file.nodes {
        let line = spanned.referenced.line();
        let raw = spanned.value;
        let kind = raw
            .kind
            .ok_or_else(|| format!("line {line}: node group without kind"))?;
        if raw.nodes.is_empty() {
            return Err(format!("line {line}: node group without nodes"));
        }
        let mut nodes = Vec::new();
        for name in raw.nodes {
            if !known.insert(name.value.clone()) {
                return Err(format!(
                    "line {}: duplicate node name {}",
                    name.referenced.line(),
                    name.value
                ));
            }
            nodes.push(name.value);
        }
        groups.push(Group {
            kind,
            image: raw.image,
            labels: raw.labels,
            nodes,
        });
    }
    let mut links = Vec::new();
    for (node, entries) in file.links {
        if !known.contains(&node.value) {
            return Err(format!(
                "line {}: link from unknown node {}",
                node.referenced.line(),
                node.value
            ));
        }
        for entry in entries {
            if !known.contains(&entry.value.node_b) {
                return Err(format!(
                    "line {}: link from {} to unknown node {}",
                    entry.referenced.line(),
                    node.value,
                    entry.value.node_b
                ));
            }
            links.push(Link {
                node: node.value.clone(),
                port: entry.value.port,
                node_b: entry.value.node_b,
                port_b: entry.value.port_b,
            });
        }
    }
    Ok(Project {
        main: file.main,
        groups,
        links,
        vars: file.vars,
    })
}

impl Project {
    pub fn node_names(&self) -> Vec<String> {
        self.groups
            .iter()
            .flat_map(|group| group.nodes.iter().cloned())
            .collect()
    }

    pub fn topology_yaml(&self) -> Result<String, String> {
        let mgmt = &self.main.mgmt;
        let mut mgmt_entries = vec![("network", text(&mgmt.network))];
        if let Some(subnet) = &mgmt.subnet {
            mgmt_entries.push(("ipv4-subnet", text(subnet)));
        }
        let mut nodes = Vec::new();
        for group in &self.groups {
            for name in &group.nodes {
                let mut node = vec![("kind", text(&group.kind)), ("image", text(&group.image))];
                if mgmt.subnet.is_some() {
                    if let Some(address) =
                        self.node_vars(name).and_then(|vars| vars.get("mgmt-ipv4"))
                    {
                        node.push(("mgmt-ipv4", address.clone()));
                    }
                }
                node.push(("startup-config", text(&format!("configs/{name}.cfg"))));
                if !group.labels.is_empty() {
                    let labels = group
                        .labels
                        .iter()
                        .map(|(key, value)| (key.clone(), text(value)))
                        .collect();
                    node.push(("labels", Ordered::Map(labels)));
                }
                nodes.push((name.clone(), map(node)));
            }
        }
        let mut topology = vec![("nodes", Ordered::Map(nodes))];
        if !self.links.is_empty() {
            let links = self
                .links
                .iter()
                .map(|link| {
                    let endpoints = vec![
                        text(&format!("{}:{}", link.node, link.port)),
                        text(&format!("{}:{}", link.node_b, link.port_b)),
                    ];
                    map(vec![("endpoints", Ordered::List(endpoints))])
                })
                .collect();
            topology.push(("links", Ordered::List(links)));
        }
        let document = map(vec![
            ("name", text(&self.main.name)),
            ("mgmt", map(mgmt_entries)),
            ("topology", map(topology)),
        ]);
        serde_saphyr::to_string(&document).map_err(|error| error.to_string())
    }

    pub fn lab_vars_yaml(&self) -> Result<String, String> {
        serde_saphyr::to_string(&self.vars).map_err(|error| error.to_string())
    }

    pub fn startup_config(&self, node: &str, renderer: &Renderer) -> Result<String, String> {
        let group = self
            .groups
            .iter()
            .find(|group| group.nodes.iter().any(|name| name == node))
            .ok_or_else(|| format!("unknown node {node}"))?;
        let file = renderer.template(&group.kind)?;
        renderer.config(&file, node, &group.kind, &self.template_vars(node))
    }

    fn node_vars(&self, node: &str) -> Option<&Ordered> {
        self.vars.get("nodes")?.get(node)
    }

    fn template_vars(&self, node: &str) -> NodeVars {
        let entry = match self.node_vars(node).map(Ordered::to_json) {
            Some(serde_json::Value::Object(entry)) => entry,
            _ => serde_json::Map::new(),
        };
        let peers = self
            .vars
            .get("observability_intent")
            .and_then(|intent| intent.get("bgp"))
            .and_then(|bgp| bgp.get("intended_peers"))
            .and_then(|peers| peers.get(node))
            .map(Ordered::to_json);
        NodeVars { entry, peers }
    }
}

pub fn outputs(dir: &Path, file: Option<&Path>) -> Result<Vec<(String, String)>, String> {
    let (name, path) = match file {
        Some(file) => (file.display().to_string(), dir.join(file)),
        None => (FILE.to_string(), dir.join(FILE)),
    };
    if !path.is_file() {
        return Err(match file {
            Some(_) => format!("no project file at {name}"),
            None => format!("no {FILE} in {}", dir.display()),
        });
    }
    let text = fs::read_to_string(&path).map_err(|error| format!("cannot read {name}: {error}"))?;
    let parsed = read(&text).map_err(|reason| format!("{name}: {reason}"))?;
    let renderer = Renderer::new(&dir.join(TEMPLATES));
    let mut outputs = vec![
        (TOPOLOGY.to_string(), parsed.topology_yaml()?),
        (
            "containerlab/lab_vars.yml".to_string(),
            parsed.lab_vars_yaml()?,
        ),
    ];
    for node in parsed.node_names() {
        let config = parsed.startup_config(&node, &renderer)?;
        outputs.push((format!("containerlab/configs/{node}.cfg"), config));
    }
    Ok(outputs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    Same,
    Differs,
    Missing,
}

pub fn compare(dir: &Path, outputs: &[(String, String)]) -> Vec<Written> {
    outputs
        .iter()
        .map(|(relative, text)| match fs::read(dir.join(relative)) {
            Ok(current) if current == text.as_bytes() => Written::Same,
            Ok(_) => Written::Differs,
            Err(_) => Written::Missing,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rendered {
    NotRendered,
    AsRendered,
    Differ { count: usize, of: usize },
    Error(String),
}

pub fn rendered(dir: &Path) -> Rendered {
    let outputs = match outputs(dir, None) {
        Ok(outputs) => outputs,
        Err(message) => return Rendered::Error(message),
    };
    let written = compare(dir, &outputs);
    let count = written.iter().filter(|w| **w != Written::Same).count();
    if written.iter().all(|w| *w == Written::Missing) {
        Rendered::NotRendered
    } else if count == 0 {
        Rendered::AsRendered
    } else {
        Rendered::Differ {
            count,
            of: written.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab_vars;
    use serde_json::json;
    use std::path::Path;

    const PROJECT: &str = include_str!("../../tests/fixtures/project-two-node/nobs_project.yml");
    const TOPOLOGY: &str =
        include_str!("../../tests/fixtures/project-two-node/expected/lab.clab.yml");
    const LAB_VARS: &str =
        include_str!("../../tests/fixtures/project-two-node/expected/lab_vars.yml");
    const SRL1: &str =
        include_str!("../../tests/fixtures/project-two-node/expected/configs/nobs-t-srl1.cfg");
    const SRL2: &str =
        include_str!("../../tests/fixtures/project-two-node/expected/configs/nobs-t-srl2.cfg");
    const BOOK_LAB_VARS: &str =
        include_str!("../../tests/fixtures/book-lab/containerlab/lab_vars.yml");

    fn fixture() -> Project {
        read(PROJECT).unwrap()
    }

    fn fixture_renderer() -> Renderer {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/project-two-node/templates");
        Renderer::new(&folder)
    }

    fn project(body: &str) -> String {
        format!("main:\n  name: nobs-t-p\n  mgmt:\n    network: nobs-t-net\n{body}vars: {{}}\n")
    }

    #[test]
    fn the_fixture_project_reads() {
        let project = fixture();
        assert_eq!(project.node_names(), ["nobs-t-srl1", "nobs-t-srl2"]);
        assert_eq!(project.main.name, "nobs-t-project");
        assert_eq!(project.main.mgmt.network, "nobs-t-net");
        assert_eq!(project.main.mgmt.subnet.as_deref(), Some("203.0.113.0/24"));
        assert_eq!(project.groups.len(), 1);
        assert_eq!(project.groups[0].kind, "nokia_srlinux");
        assert_eq!(project.links.len(), 1);
        let link = &project.links[0];
        assert_eq!(
            (
                &link.node[..],
                &link.port[..],
                &link.node_b[..],
                &link.port_b[..]
            ),
            ("nobs-t-srl1", "e1-1", "nobs-t-srl2", "e1-1")
        );
    }

    #[test]
    fn a_link_to_an_unknown_node_is_an_error_naming_its_line() {
        let text = project(
            "nodes:\n  - kind: nokia_srlinux\n    image: nobs-t-image\n    nodes: [nobs-t-srl1, nobs-t-srl2]\nlinks:\n  nobs-t-srl1:\n    - {port: e1-1, node_b: nobs-t-srl2, port_b: e1-1}\n    - {port: e1-2, node_b: nobs-t-srl9, port_b: e1-1}\n",
        );
        assert_eq!(
            read(&text).unwrap_err(),
            "line 12: link from nobs-t-srl1 to unknown node nobs-t-srl9"
        );
    }

    #[test]
    fn a_link_from_an_unknown_node_is_an_error_naming_its_line() {
        let text = project(
            "nodes:\n  - kind: nokia_srlinux\n    image: nobs-t-image\n    nodes: [nobs-t-srl1]\nlinks:\n  nobs-t-srl9:\n    - {port: e1-1, node_b: nobs-t-srl1, port_b: e1-1}\n",
        );
        assert_eq!(
            read(&text).unwrap_err(),
            "line 10: link from unknown node nobs-t-srl9"
        );
    }

    #[test]
    fn a_node_group_without_kind_is_an_error_naming_its_line() {
        let text = project(
            "nodes:\n  - kind: nokia_srlinux\n    image: nobs-t-image\n    nodes: [nobs-t-srl1]\n  - image: nobs-t-image\n    nodes: [nobs-t-srl2]\n",
        );
        assert_eq!(read(&text).unwrap_err(), "line 9: node group without kind");
    }

    #[test]
    fn a_node_group_without_nodes_is_an_error_naming_its_line() {
        let text =
            project("nodes:\n  - kind: nokia_srlinux\n    image: nobs-t-image\n    nodes: []\n");
        assert_eq!(read(&text).unwrap_err(), "line 6: node group without nodes");
    }

    #[test]
    fn a_duplicate_node_name_is_an_error_naming_its_line() {
        let text = project(
            "nodes:\n  - kind: nokia_srlinux\n    image: nobs-t-image\n    nodes:\n      - nobs-t-srl1\n      - nobs-t-srl2\n  - kind: ceos\n    image: nobs-t-image\n    nodes: [nobs-t-srl1]\n",
        );
        assert_eq!(
            read(&text).unwrap_err(),
            "line 13: duplicate node name nobs-t-srl1"
        );
    }

    #[test]
    fn the_fixture_project_gives_the_expected_topology_byte_for_byte() {
        let written = fixture().topology_yaml().unwrap();
        assert_eq!(written, TOPOLOGY);
        let value: serde_json::Value = serde_saphyr::from_str(&written).unwrap();
        let nodes = &value["topology"]["nodes"];
        assert_eq!(nodes.as_object().unwrap().len(), 2);
        for name in ["nobs-t-srl1", "nobs-t-srl2"] {
            assert_eq!(nodes[name]["kind"], "nokia_srlinux");
            assert_eq!(nodes[name]["image"], "ghcr.io/nokia/srlinux:latest");
            assert_eq!(nodes[name]["startup-config"], format!("configs/{name}.cfg"));
            assert_eq!(
                nodes[name]["labels"]["nobs-t-label"],
                "${NOBS_T_LABEL:-unset}"
            );
        }
        assert_eq!(value["mgmt"]["ipv4-subnet"], "203.0.113.0/24");
        assert_eq!(
            value["topology"]["links"],
            json!([{"endpoints": ["nobs-t-srl1:e1-1", "nobs-t-srl2:e1-1"]}])
        );
    }

    #[test]
    fn the_written_lab_vars_read_as_the_book_lab_file_and_equal_the_expected_text() {
        let written = fixture().lab_vars_yaml().unwrap();
        assert_eq!(
            lab_vars::read(&written).unwrap(),
            lab_vars::read(BOOK_LAB_VARS).unwrap()
        );
        assert_eq!(written, LAB_VARS);
        let again: Ordered = serde_saphyr::from_str(&written).unwrap();
        assert_eq!(serde_saphyr::to_string(&again).unwrap(), written);
    }

    #[test]
    fn the_template_data_from_the_project_equals_the_data_read_back_from_the_written_lab_vars() {
        let text = "main:\n  name: nobs-t-p\n  mgmt:\n    network: nobs-t-net\nnodes:\n  - kind: nokia_srlinux\n    image: nobs-t-image\n    nodes: [nobs-t-srl1, nobs-t-srl2]\nvars:\n  nodes:\n    nobs-t-srl1:\n      mgmt-ipv4: 203.0.113.11\n      nobs-t-code: \"65001\"\n      nobs-t-number: 65001\n      nobs-t-weight: 1.5\n      nobs-t-flag: true\n      interfaces:\n        - name: ethernet-1/1\n          ipv4: [192.0.2.0/31, 198.51.100.0/31]\n    nobs-t-srl2:\n      mgmt-ipv4: 203.0.113.12\n  observability_intent:\n    bgp:\n      intended_peers:\n        nobs-t-srl1:\n          - peer_ip: 192.0.2.1\n            remote_as: 65002\n            nobs-t-note: \"1.5\"\n        nobs-t-srl2:\n";
        let project = read(text).unwrap();
        let lab = lab_vars::read(&project.lab_vars_yaml().unwrap()).unwrap();
        assert_eq!(lab.devices.len(), 2);
        for device in &lab.devices {
            assert_eq!(project.template_vars(&device.name), device.vars);
        }
        let srl1 = &lab.devices[0].vars;
        assert_eq!(srl1.entry["nobs-t-code"], json!("65001"));
        assert_eq!(srl1.entry["nobs-t-number"], json!(65001));
        assert_eq!(srl1.entry["nobs-t-weight"], json!(1.5));
        assert_eq!(srl1.entry["nobs-t-flag"], json!(true));
        assert_eq!(
            srl1.entry["interfaces"][0]["ipv4"],
            json!(["192.0.2.0/31", "198.51.100.0/31"])
        );
        assert_eq!(srl1.peers.as_ref().unwrap()[0]["nobs-t-note"], json!("1.5"));
        assert_eq!(lab.devices[1].vars.peers, Some(serde_json::Value::Null));
    }

    #[test]
    fn the_startup_config_sees_every_template_name_and_equals_the_intended_config() {
        let text = "main:\n  name: nobs-t-p\n  mgmt:\n    network: nobs-t-net\nnodes:\n  - kind: nokia_srlinux\n    image: nobs-t-image\n    nodes: [nobs-t-srl1]\nvars:\n  nodes:\n    nobs-t-srl1:\n      mgmt-ipv4: 203.0.113.11\n      nobs_t_site: nobs-t-rack1\n  observability_intent:\n    bgp:\n      intended_peers:\n        nobs-t-srl1:\n          - peer_ip: 192.0.2.1\n";
        let folder = Folder::new("names");
        folder.write(
            "templates/srl.j2",
            "{{ mgmt_ipv4 }} {{ nobs_t_site }} {% for s in sessions %}{{ s.peer_ip }}{% endfor %}\n",
        );
        let project = read(text).unwrap();
        let startup = project
            .startup_config("nobs-t-srl1", &Renderer::new(&folder.0.join(TEMPLATES)))
            .unwrap();
        assert_eq!(startup, "203.0.113.11 nobs-t-rack1 192.0.2.1");
        let source = crate::sot::Fixed::from_lab_vars(
            &lab_vars::read(&project.lab_vars_yaml().unwrap()).unwrap(),
        );
        assert_eq!(
            crate::intended::intended(&folder.0, "nobs-t-srl1", "nokia_srlinux", Ok(&source)),
            Ok(startup)
        );
    }

    #[test]
    fn srl1_renders_the_expected_startup_config() {
        let project = fixture();
        let renderer = fixture_renderer();
        assert_eq!(
            project.startup_config("nobs-t-srl1", &renderer).unwrap(),
            SRL1
        );
        assert_eq!(
            project.startup_config("nobs-t-srl2", &renderer).unwrap(),
            SRL2
        );
    }

    #[test]
    fn a_kind_without_a_template_is_an_error_naming_the_kind_and_the_file() {
        let folder =
            std::env::temp_dir().join(format!("nobs-project-empty-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let result = fixture().startup_config("nobs-t-srl1", &Renderer::new(&folder));
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(
            result.unwrap_err(),
            "no template for kind nokia_srlinux: expected templates/srl.j2"
        );
    }

    #[test]
    fn a_node_the_project_does_not_name_is_an_error() {
        let result = fixture().startup_config("nobs-t-srl9", &fixture_renderer());
        assert_eq!(result.unwrap_err(), "unknown node nobs-t-srl9");
    }

    #[test]
    fn an_unknown_top_level_key_is_an_error() {
        let text = "main:\n  name: nobs-t-p\n  mgmt:\n    network: nobs-t-net\nnodes:\n  - kind: nokia_srlinux\n    image: nobs-t-image\n    nodes: [nobs-t-srl1, nobs-t-srl2]\nlink:\n  nobs-t-srl1:\n    - {port: e1-1, node_b: nobs-t-srl2, port_b: e1-1}\nvars: {}\n";
        let error = read(text).unwrap_err();
        assert!(error.contains("link"), "{error}");
    }

    struct Folder(std::path::PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("nobs-project-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let folder = Self(root);
            folder.write(FILE, PROJECT);
            folder.write(
                "templates/srl.j2",
                include_str!("../../tests/fixtures/project-two-node/templates/srl.j2"),
            );
            folder
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn compare_and_rendered_name_each_state_of_the_written_files() {
        use Written::{Differs, Missing, Same};
        let folder = Folder::new("states");
        let dir = folder.0.as_path();
        let outputs = outputs(dir, None).unwrap();
        assert_eq!(outputs[0].0, super::TOPOLOGY);

        assert_eq!(compare(dir, &outputs), [Missing; 4]);
        assert_eq!(rendered(dir), Rendered::NotRendered);

        for (relative, text) in &outputs {
            folder.write(relative, text);
        }
        assert_eq!(compare(dir, &outputs), [Same; 4]);
        assert_eq!(rendered(dir), Rendered::AsRendered);

        folder.write("containerlab/lab_vars.yml", "extra: 1\n");
        assert_eq!(compare(dir, &outputs), [Same, Differs, Same, Same]);
        assert_eq!(rendered(dir), Rendered::Differ { count: 1, of: 4 });

        fs::remove_file(dir.join("containerlab/configs/nobs-t-srl2.cfg")).unwrap();
        assert_eq!(compare(dir, &outputs), [Same, Differs, Same, Missing]);
        assert_eq!(rendered(dir), Rendered::Differ { count: 2, of: 4 });

        folder.write(FILE, "main: [\n");
        let error = super::outputs(dir, None).unwrap_err();
        assert!(error.starts_with("nobs_project.yml: "), "{error}");
        assert_eq!(rendered(dir), Rendered::Error(error));
    }

    #[test]
    fn an_unknown_key_under_mgmt_is_an_error() {
        let text = "main:\n  name: nobs-t-p\n  mgmt:\n    network: nobs-t-net\n    subnett: 203.0.113.0/24\nnodes:\n  - kind: nokia_srlinux\n    image: nobs-t-image\n    nodes: [nobs-t-srl1]\nvars: {}\n";
        let error = read(text).unwrap_err();
        assert!(error.contains("subnett"), "{error}");
    }
}
