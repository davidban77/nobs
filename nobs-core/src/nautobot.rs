use std::collections::HashMap;
use std::fmt;
use std::net::Ipv4Addr;
use std::time::Duration;

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use ureq::typestate::WithBody;
use ureq::{Agent, Body, RequestBuilder};

use crate::http::{self, Reply};

const TIMEOUT: Duration = Duration::from_secs(10);
const PAGE: usize = 200;
const ROLE_NAME: &str = "network_device";
const SITE: &str = "site";
const LAB: &str = "lab";
const NAMESPACE: &str = "lab-default";
const ACTIVE: &str = "lab-active";
const ALERTED: &str = "Alerted";
const FIELD_KEY: &str = "maintenance";
const MGMT_PREFIX: &str = "lab-mgmt-prefix";
const MGMT_INTERFACE: &str = "Management0";
const TYPES: [(&str, &str); 2] = [("Arista", "cEOS"), ("Nokia", "SRLinux")];
const STATUS_CONTENT_TYPES: [&str; 5] = [
    "dcim.device",
    "dcim.interface",
    "dcim.location",
    "ipam.ipaddress",
    "ipam.prefix",
];

#[derive(Debug)]
pub enum Error {
    Input(String),
    Http {
        method: String,
        path: String,
        status: u16,
        body: String,
    },
    Transport(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Input(reason) => f.write_str(reason),
            Error::Http {
                method,
                path,
                status,
                body,
            } => {
                let line = body.lines().next().unwrap_or("");
                f.write_str(format!("{method} {path} failed: {status} {line}").trim_end())
            }
            Error::Transport(text) => f.write_str(text),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Updated,
}

#[derive(Debug)]
pub struct Load {
    pub rows: Vec<(String, usize, usize)>,
}

#[derive(Debug)]
pub struct Deleted {
    pub rows: Vec<(String, usize)>,
}

#[derive(Clone, Copy)]
enum Kind {
    Role,
    Manufacturer,
    DeviceType,
    LocationType,
    Status,
    CustomField,
    Location,
    Namespace,
    Prefix,
    Device,
    Interface,
    IpAddress,
    Mapping,
}

impl Kind {
    const ALL: [Kind; 13] = [
        Kind::Role,
        Kind::Manufacturer,
        Kind::DeviceType,
        Kind::LocationType,
        Kind::Status,
        Kind::CustomField,
        Kind::Location,
        Kind::Namespace,
        Kind::Prefix,
        Kind::Device,
        Kind::Interface,
        Kind::IpAddress,
        Kind::Mapping,
    ];

    fn word(self) -> &'static str {
        match self {
            Kind::Role => "role",
            Kind::Manufacturer => "manufacturer",
            Kind::DeviceType => "device type",
            Kind::LocationType => "location type",
            Kind::Status => "status",
            Kind::CustomField => "custom field",
            Kind::Location => "location",
            Kind::Namespace => "namespace",
            Kind::Prefix => "prefix",
            Kind::Device => "device",
            Kind::Interface => "interface",
            Kind::IpAddress => "ip address",
            Kind::Mapping => "ip address to interface",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Kind::Role => "/api/extras/roles/",
            Kind::Manufacturer => "/api/dcim/manufacturers/",
            Kind::DeviceType => "/api/dcim/device-types/",
            Kind::LocationType => "/api/dcim/location-types/",
            Kind::Status => "/api/extras/statuses/",
            Kind::CustomField => "/api/extras/custom-fields/",
            Kind::Location => "/api/dcim/locations/",
            Kind::Namespace => "/api/ipam/namespaces/",
            Kind::Prefix => "/api/ipam/prefixes/",
            Kind::Device => "/api/dcim/devices/",
            Kind::Interface => "/api/dcim/interfaces/",
            Kind::IpAddress => "/api/ipam/ip-addresses/",
            Kind::Mapping => "/api/ipam/ip-address-to-interface/",
        }
    }
}

pub fn device_type(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "ceos" | "arista_ceos" => Some(TYPES[0]),
        "nokia_srlinux" | "srl" => Some(TYPES[1]),
        _ => None,
    }
}

pub struct Nautobot {
    agent: Agent,
    url: String,
    token: String,
}

impl Nautobot {
    pub fn new(url: &str, token: &str) -> Self {
        Self {
            agent: http::agent(TIMEOUT),
            url: url.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    pub fn load(
        &self,
        topology_text: &str,
        lab_vars_text: &str,
        progress: &mut dyn FnMut(&str, &str, Outcome),
    ) -> Result<Load, Error> {
        let plan = Plan::read(topology_text, lab_vars_text)?;
        let mut run = Run {
            nautobot: self,
            progress,
            rows: Kind::ALL
                .iter()
                .map(|kind| (kind.word().to_string(), 0, 0))
                .collect(),
        };
        let role = run.upsert(
            Kind::Role,
            &[("name", ROLE_NAME)],
            ROLE_NAME,
            json!({"name": ROLE_NAME, "content_types": ["dcim.device"]}),
            None,
        )?;
        let mut types = HashMap::new();
        for (manufacturer, model) in TYPES {
            let manufacturer_id = run.upsert(
                Kind::Manufacturer,
                &[("name", manufacturer)],
                manufacturer,
                json!({"name": manufacturer}),
                None,
            )?;
            let id = run.upsert(
                Kind::DeviceType,
                &[("manufacturer", &manufacturer_id), ("model", model)],
                model,
                json!({"manufacturer": manufacturer, "model": model}),
                None,
            )?;
            types.insert(model, id);
        }
        let site = run.upsert(
            Kind::LocationType,
            &[("name", SITE)],
            SITE,
            json!({"name": SITE, "content_types": ["dcim.device"]}),
            None,
        )?;
        let active = run.status(ACTIVE, "aaf0d1")?;
        run.status(ALERTED, "ff5a36")?;
        run.custom_field(json!({
            "label": "Maintenance",
            "key": FIELD_KEY,
            "type": "boolean",
            "default": false,
            "required": false,
            "content_types": ["dcim.device"],
        }))?;
        let location = run.upsert(
            Kind::Location,
            &[("name", LAB)],
            LAB,
            json!({"name": LAB, "location_type": {"id": site}, "status": {"id": active}}),
            None,
        )?;
        let namespace = run.upsert(
            Kind::Namespace,
            &[("name", NAMESPACE)],
            NAMESPACE,
            json!({"name": NAMESPACE}),
            None,
        )?;
        let lab = Lab {
            role,
            location,
            namespace,
            active,
            types,
        };
        for (prefix, description) in &plan.prefixes {
            run.upsert(
                Kind::Prefix,
                &[("prefix", prefix), ("namespace", &lab.namespace)],
                prefix,
                json!({
                    "prefix": prefix,
                    "namespace": {"id": lab.namespace},
                    "type": "network",
                    "status": {"id": lab.active},
                    "description": description,
                }),
                None,
            )?;
        }
        for node in &plan.nodes {
            let device = run.upsert(
                Kind::Device,
                &[("name", &node.name)],
                &node.name,
                json!({
                    "name": node.name,
                    "role": {"id": lab.role},
                    "device_type": {"id": lab.types[node.model]},
                    "location": {"id": lab.location},
                    "status": {"id": lab.active},
                    "local_config_context_data": {
                        "observability_intent": {
                            "bgp": {"afi_safi": plan.afi_safi, "intended_peers": node.peers}
                        }
                    },
                }),
                Some(("custom_fields", json!({FIELD_KEY: false}))),
            )?;
            for interface in &node.interfaces {
                let id = run.interface(
                    &lab,
                    &device,
                    &interface.name,
                    &format!("Interface {}", interface.name),
                    &interface.role,
                )?;
                for address in &interface.addresses {
                    let ip = run.ip_address(&lab, address)?;
                    run.map(&ip, address, &id, &interface.name)?;
                }
            }
            let ip = run.ip_address(&lab, &node.mgmt)?;
            let id = run.interface(
                &lab,
                &device,
                MGMT_INTERFACE,
                "Management Interface",
                "mgmt",
            )?;
            run.map(&ip, &node.mgmt, &id, MGMT_INTERFACE)?;
            self.patch(
                &format!("{}{device}/", Kind::Device.path()),
                &json!({"primary_ip4": {"id": ip}}),
            )?;
        }
        Ok(Load { rows: run.rows })
    }

    pub fn delete(&self, progress: &mut dyn FnMut(&str, usize)) -> Result<Deleted, Error> {
        let locations = self.find_all(Kind::Location, &[("name", LAB)])?;
        let namespaces = self.find_all(Kind::Namespace, &[("name", NAMESPACE)])?;
        let devices = self.find_in(Kind::Device, "location", &locations)?;
        let prefixes = self.find_in(Kind::Prefix, "namespace", &namespaces)?;
        let addresses = self.find_in(Kind::IpAddress, "namespace", &namespaces)?;
        let mut mappings = Vec::new();
        for device in &devices {
            for interface in self.find_all(Kind::Interface, &[("device", id(device))])? {
                mappings.extend(self.find_all(Kind::Mapping, &[("interface", id(&interface))])?);
            }
        }
        let mut found = vec![
            (Kind::Mapping, ids(&mappings)),
            (Kind::IpAddress, ids(&addresses)),
            (Kind::Device, ids(&devices)),
            (Kind::Prefix, ids(&prefixes)),
            (Kind::Namespace, ids(&namespaces)),
            (Kind::Location, ids(&locations)),
        ];
        found.extend(self.find_removed(removed_before_field())?);
        found.push((Kind::CustomField, ids(&self.find_custom_fields()?)));
        let (device_types, manufacturers) = self.find_device_types()?;
        found.push((Kind::DeviceType, ids(&device_types)));
        found.push((Kind::Manufacturer, ids(&manufacturers)));
        found.extend(self.find_removed(removed_after_field())?);
        let mut rows = Vec::new();
        for (kind, kind_ids) in found {
            if !kind_ids.is_empty() {
                self.remove(kind.path(), &kind_ids)?;
            }
            progress(kind.word(), kind_ids.len());
            rows.push((kind.word().to_string(), kind_ids.len()));
        }
        Ok(Deleted { rows })
    }

    fn find_all(&self, kind: Kind, filter: &[(&str, &str)]) -> Result<Vec<Value>, Error> {
        let page = PAGE.to_string();
        let mut filter = filter.to_vec();
        filter.push(("limit", &page));
        let mut path = format!("{}{}", kind.path(), query(&filter));
        let mut items = Vec::new();
        loop {
            let reply = self.get(&path)?;
            for item in results(&reply, &path)? {
                field(item, "id", &path)?;
                items.push(item.clone());
            }
            match reply.get("next").and_then(Value::as_str) {
                Some(next) => path = path_and_query(next),
                None => return Ok(items),
            }
        }
    }

    fn find_in(&self, kind: Kind, parent: &str, parents: &[Value]) -> Result<Vec<Value>, Error> {
        let mut items = Vec::new();
        for found in parents {
            items.extend(self.find_all(kind, &[(parent, id(found))])?);
        }
        Ok(items)
    }

    fn find_removed(
        &self,
        removed: Vec<(Kind, Vec<Filter>)>,
    ) -> Result<Vec<(Kind, Vec<String>)>, Error> {
        let mut found = Vec::new();
        for (kind, filters) in removed {
            let mut kind_ids = Vec::new();
            for filter in filters {
                kind_ids.extend(ids(&self.find_all(kind, &filter)?));
            }
            found.push((kind, kind_ids));
        }
        Ok(found)
    }

    fn find_device_types(&self) -> Result<(Vec<Value>, Vec<Value>), Error> {
        let mut device_types = Vec::new();
        let mut manufacturers = Vec::new();
        for (name, model) in TYPES {
            let found = self.find_all(Kind::Manufacturer, &[("name", name)])?;
            for manufacturer in &found {
                device_types.extend(self.find_all(
                    Kind::DeviceType,
                    &[("manufacturer", id(manufacturer)), ("model", model)],
                )?);
            }
            manufacturers.extend(found);
        }
        Ok((device_types, manufacturers))
    }

    fn find_custom_fields(&self) -> Result<Vec<Value>, Error> {
        Ok(self
            .find_all(Kind::CustomField, &[])?
            .into_iter()
            .filter(|item| item["key"] == FIELD_KEY)
            .collect())
    }

    fn get(&self, path: &str) -> Result<Value, Error> {
        let request = self.agent.get(format!("{}{path}", self.url));
        let reply = self.finish("GET", path, self.with_headers(request).call())?;
        parse("GET", path, &reply)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, Error> {
        let request = self.agent.post(format!("{}{path}", self.url));
        let reply = self.send("POST", path, request, body)?;
        parse("POST", path, &reply)
    }

    fn patch(&self, path: &str, body: &Value) -> Result<Value, Error> {
        let request = self.agent.patch(format!("{}{path}", self.url));
        let reply = self.send("PATCH", path, request, body)?;
        parse("PATCH", path, &reply)
    }

    fn remove(&self, path: &str, ids: &[String]) -> Result<(), Error> {
        let request = self
            .agent
            .delete(format!("{}{path}", self.url))
            .force_send_body();
        let body: Vec<Value> = ids.iter().map(|id| json!({"id": id})).collect();
        self.send("DELETE", path, request, &Value::Array(body))
            .map(|_| ())
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        request: RequestBuilder<WithBody>,
        body: &Value,
    ) -> Result<Reply, Error> {
        let sent = self
            .with_headers(request)
            .header("Content-Type", "application/json")
            .send(body.to_string().as_bytes());
        self.finish(method, path, sent)
    }

    fn with_headers<B>(&self, request: RequestBuilder<B>) -> RequestBuilder<B> {
        request
            .header("Authorization", format!("Token {}", self.token))
            .header("Accept", "application/json")
    }

    fn finish(
        &self,
        method: &str,
        path: &str,
        sent: Result<ureq::http::Response<Body>, ureq::Error>,
    ) -> Result<Reply, Error> {
        let reply = http::read(&self.url, sent).map_err(Error::Transport)?;
        if (200..300).contains(&reply.status) {
            Ok(reply)
        } else {
            Err(Error::Http {
                method: method.to_string(),
                path: path.to_string(),
                status: reply.status,
                body: reply.body,
            })
        }
    }
}

struct Run<'a> {
    nautobot: &'a Nautobot,
    progress: &'a mut dyn FnMut(&str, &str, Outcome),
    rows: Vec<(String, usize, usize)>,
}

struct Lab {
    role: String,
    location: String,
    namespace: String,
    active: String,
    types: HashMap<&'static str, String>,
}

impl Run<'_> {
    fn upsert(
        &mut self,
        kind: Kind,
        filter: &[(&str, &str)],
        key: &str,
        body: Value,
        on_create: Option<(&str, Value)>,
    ) -> Result<String, Error> {
        let path = format!("{}{}", kind.path(), query(filter));
        let found = self.nautobot.get(&path)?;
        self.write(kind, key, results(&found, &path)?, &path, body, on_create)
    }

    fn custom_field(&mut self, body: Value) -> Result<(), Error> {
        let found = self.nautobot.find_custom_fields()?;
        let path = Kind::CustomField.path();
        self.write(Kind::CustomField, FIELD_KEY, &found, path, body, None)
            .map(|_| ())
    }

    fn write(
        &mut self,
        kind: Kind,
        key: &str,
        found: &[Value],
        path: &str,
        mut body: Value,
        on_create: Option<(&str, Value)>,
    ) -> Result<String, Error> {
        let (written, outcome) = match found {
            [] => {
                if let Some((name, value)) = on_create {
                    body[name] = value;
                }
                (self.nautobot.post(kind.path(), &body)?, Outcome::Created)
            }
            [one] => {
                let found_id = field(one, "id", path)?;
                let update = format!("{}{found_id}/", kind.path());
                (self.nautobot.patch(&update, &body)?, Outcome::Updated)
            }
            many => {
                return Err(Error::Input(format!(
                    "{} {key}: {} objects match in Nautobot",
                    kind.word(),
                    many.len()
                )))
            }
        };
        let written_id = field(&written, "id", kind.path())?.to_string();
        let display = written
            .get("display")
            .and_then(Value::as_str)
            .unwrap_or(key);
        let row = &mut self.rows[kind as usize];
        match outcome {
            Outcome::Created => row.1 += 1,
            Outcome::Updated => row.2 += 1,
        }
        (self.progress)(kind.word(), display, outcome);
        Ok(written_id)
    }

    fn status(&mut self, name: &str, color: &str) -> Result<String, Error> {
        self.upsert(
            Kind::Status,
            &[("name", name)],
            name,
            json!({"name": name, "content_types": STATUS_CONTENT_TYPES, "color": color}),
            None,
        )
    }

    fn interface(
        &mut self,
        lab: &Lab,
        device: &str,
        name: &str,
        description: &str,
        label: &str,
    ) -> Result<String, Error> {
        self.upsert(
            Kind::Interface,
            &[("device", device), ("name", name)],
            name,
            json!({
                "device": {"id": device},
                "name": name,
                "type": "virtual",
                "enabled": true,
                "description": description,
                "status": {"id": lab.active},
                "label": label,
            }),
            None,
        )
    }

    fn ip_address(&mut self, lab: &Lab, address: &str) -> Result<String, Error> {
        self.upsert(
            Kind::IpAddress,
            &[("address", address), ("namespace", &lab.namespace)],
            address,
            json!({
                "address": address,
                "status": {"id": lab.active},
                "namespace": {"id": lab.namespace},
                "type": "host",
            }),
            None,
        )
    }

    fn map(&mut self, ip: &str, address: &str, interface: &str, name: &str) -> Result<(), Error> {
        self.upsert(
            Kind::Mapping,
            &[("ip_address", ip), ("interface", interface)],
            &format!("{address} on {name}"),
            json!({"ip_address": {"id": ip}, "interface": {"id": interface}}),
            None,
        )
        .map(|_| ())
    }
}

struct Plan {
    prefixes: Vec<(String, String)>,
    nodes: Vec<Node>,
    afi_safi: Value,
}

struct Node {
    name: String,
    model: &'static str,
    mgmt: String,
    interfaces: Vec<Interface>,
    peers: Value,
}

struct Interface {
    name: String,
    role: String,
    addresses: Vec<String>,
}

struct Ordered(Vec<(String, Value)>);

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;

        impl<'de> Visitor<'de> for Entries {
            type Value = Ordered;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map from node name to its settings")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(Ordered(entries))
            }
        }

        deserializer.deserialize_map(Entries)
    }
}

#[derive(Deserialize)]
struct Topology {
    mgmt: Option<Mgmt>,
    topology: Option<Section>,
}

#[derive(Deserialize)]
struct Mgmt {
    #[serde(rename = "ipv4-subnet")]
    subnet: Option<String>,
}

#[derive(Deserialize)]
struct Section {
    nodes: Option<Ordered>,
    defaults: Option<Defaults>,
}

#[derive(Deserialize)]
struct Defaults {
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Vars {
    prefixes: Option<Vec<RawPrefix>>,
    nodes: Option<Ordered>,
    observability_intent: Option<Value>,
}

#[derive(Deserialize)]
struct RawPrefix {
    prefix: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct RawNode {
    kind: Option<String>,
    #[serde(rename = "mgmt-ipv4")]
    mgmt: Option<String>,
    interfaces: Option<Vec<RawInterface>>,
}

#[derive(Deserialize)]
struct RawInterface {
    name: Option<String>,
    role: Option<String>,
    ipv4: Option<Address>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Address {
    One(String),
    Many(Vec<String>),
}

impl Plan {
    fn read(topology_text: &str, vars_text: &str) -> Result<Plan, Error> {
        let topology: Topology = yaml(topology_text, "topology")?;
        let vars: Vars = yaml(vars_text, "lab_vars")?;
        let section = topology
            .topology
            .ok_or_else(|| input("topology has no topology section"))?;
        let nodes = section
            .nodes
            .ok_or_else(|| input("topology has no topology.nodes"))?
            .0;
        let subnet = topology
            .mgmt
            .and_then(|mgmt| mgmt.subnet)
            .ok_or_else(|| input("topology has no mgmt.ipv4-subnet"))?;
        if !is_ipv4(&subnet, true) {
            return Err(input(format!(
                "mgmt.ipv4-subnet {subnet} is not an IPv4 prefix"
            )));
        }
        let mut prefixes = Vec::new();
        let listed = vars
            .prefixes
            .ok_or_else(|| input("lab_vars has no prefixes"))?;
        for (at, entry) in listed.into_iter().enumerate() {
            let (Some(prefix), Some(name)) = (entry.prefix, entry.name) else {
                return Err(input(format!(
                    "lab_vars prefixes[{at}] needs a prefix and a name"
                )));
            };
            if !is_ipv4(&prefix, true) {
                return Err(input(format!(
                    "lab_vars prefixes[{at}]: {prefix} is not an IPv4 prefix"
                )));
            }
            prefixes.push((prefix, name));
        }
        prefixes.push((subnet, MGMT_PREFIX.to_string()));
        let described = vars.nodes.ok_or_else(|| input("lab_vars has no nodes"))?.0;
        if let Some((name, _)) = described
            .iter()
            .find(|(name, _)| !nodes.iter().any(|(known, _)| known == name))
        {
            return Err(input(format!(
                "lab_vars node {name} is not in the topology"
            )));
        }
        let bgp = vars
            .observability_intent
            .as_ref()
            .and_then(|intent| intent.get("bgp"));
        let afi_safi = bgp
            .and_then(|bgp| bgp.get("afi_safi"))
            .cloned()
            .unwrap_or(Value::Null);
        let mut planned = Vec::new();
        for (name, own) in nodes {
            let Some((_, extra)) = described.iter().find(|(known, _)| *known == name) else {
                return Err(input(format!(
                    "topology node {name} has no entry in lab_vars"
                )));
            };
            let mut merged = settings(own, &name, "topology")?;
            merged.extend(settings(extra.clone(), &name, "lab_vars")?);
            let raw: RawNode = serde_json::from_value(Value::Object(merged))
                .map_err(|error| input(format!("node {name}: {error}")))?;
            let kind = raw
                .kind
                .or_else(|| section.defaults.as_ref().and_then(|d| d.kind.clone()))
                .ok_or_else(|| input(format!("node {name} has no kind")))?;
            let (_, model) = device_type(&kind)
                .ok_or_else(|| input(format!("node {name}: kind {kind} has no device type")))?;
            let mgmt = raw
                .mgmt
                .ok_or_else(|| input(format!("node {name} has no mgmt-ipv4")))?;
            if !is_ipv4(&mgmt, false) {
                return Err(input(format!(
                    "node {name}: mgmt-ipv4 {mgmt} is not an IPv4 address"
                )));
            }
            let listed = raw
                .interfaces
                .ok_or_else(|| input(format!("node {name} has no interfaces")))?;
            let mut interfaces = Vec::new();
            for (at, interface) in listed.into_iter().enumerate() {
                let (Some(interface_name), Some(role)) = (interface.name, interface.role) else {
                    return Err(input(format!(
                        "node {name}: interface {at} needs a name and a role"
                    )));
                };
                let addresses = match interface.ipv4 {
                    None => Vec::new(),
                    Some(Address::One(address)) => vec![address],
                    Some(Address::Many(addresses)) => addresses,
                };
                if let Some(bad) = addresses.iter().find(|address| !is_ipv4(address, false)) {
                    return Err(input(format!(
                        "node {name} interface {interface_name}: {bad} is not an IPv4 address"
                    )));
                }
                interfaces.push(Interface {
                    name: interface_name,
                    role,
                    addresses,
                });
            }
            let peers = bgp
                .and_then(|bgp| bgp.get("intended_peers"))
                .and_then(|peers| peers.get(&name))
                .filter(|peers| !peers.is_null())
                .cloned()
                .unwrap_or_else(|| json!([]));
            planned.push(Node {
                name,
                model,
                mgmt,
                interfaces,
                peers,
            });
        }
        Ok(Plan {
            prefixes,
            nodes: planned,
            afi_safi,
        })
    }
}

fn input(text: impl Into<String>) -> Error {
    Error::Input(text.into())
}

fn yaml<T: for<'de> Deserialize<'de>>(text: &str, file: &str) -> Result<T, Error> {
    serde_saphyr::from_str(text).map_err(|error| input(format!("{file}: {error}")))
}

fn settings(value: Value, node: &str, file: &str) -> Result<Map<String, Value>, Error> {
    match value {
        Value::Null => Ok(Map::new()),
        Value::Object(map) => Ok(map),
        _ => Err(input(format!("{file} node {node} is not a mapping"))),
    }
}

fn is_ipv4(text: &str, needs_length: bool) -> bool {
    match text.split_once('/') {
        Some((address, length)) => {
            address.parse::<Ipv4Addr>().is_ok() && length.parse::<u8>().is_ok_and(|n| n <= 32)
        }
        None => !needs_length && text.parse::<Ipv4Addr>().is_ok(),
    }
}

type Filter = Vec<(&'static str, &'static str)>;

fn removed_before_field() -> Vec<(Kind, Vec<Filter>)> {
    vec![
        (Kind::LocationType, vec![vec![("name", SITE)]]),
        (
            Kind::Status,
            vec![vec![("name", ACTIVE)], vec![("name", ALERTED)]],
        ),
    ]
}

fn removed_after_field() -> Vec<(Kind, Vec<Filter>)> {
    vec![(Kind::Role, vec![vec![("name", ROLE_NAME)]])]
}

fn query(filter: &[(&str, &str)]) -> String {
    let pairs: Vec<String> = filter
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect();
    format!("?{}", pairs.join("&"))
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn path_and_query(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.find('/')
        .map_or_else(|| "/".to_string(), |at| rest[at..].to_string())
}

fn parse(method: &str, path: &str, reply: &Reply) -> Result<Value, Error> {
    serde_json::from_str(&reply.body)
        .map_err(|_| Error::Transport(format!("{method} {path}: the reply is not JSON")))
}

fn results<'a>(reply: &'a Value, path: &str) -> Result<&'a [Value], Error> {
    reply
        .get("results")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| bad_reply(path, "results"))
}

fn field<'a>(value: &'a Value, name: &str, path: &str) -> Result<&'a str, Error> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| bad_reply(path, name))
}

fn bad_reply(path: &str, name: &str) -> Error {
    Error::Transport(format!("{path}: the reply has no {name}"))
}

fn id(item: &Value) -> &str {
    item["id"].as_str().unwrap_or_default()
}

fn ids(items: &[Value]) -> Vec<String> {
    items.iter().map(|item| id(item).to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::stub::Stub;

    const TOKEN: &str = "nobs-t-token";
    const FIXTURE_TOPOLOGY: &str =
        include_str!("../../tests/fixtures/book-lab/containerlab/lab.yml");
    const FIXTURE_VARS: &str =
        include_str!("../../tests/fixtures/book-lab/containerlab/lab_vars.yml");
    const NO_RESULTS: &str = r#"{"count":0,"next":null,"results":[]}"#;
    const NOT_AN_ID: &str = r#"{"manufacturer":["Select a valid choice. Arista is not one of the available choices."]}"#;

    type Call = (String, String, Value);

    fn topology() -> String {
        FIXTURE_TOPOLOGY.replace(
            "  network: nobs-t-net\n",
            "  network: nobs-t-net\n  ipv4-subnet: 203.0.113.0/24\n",
        )
    }

    fn texts(nodes: &[(&str, &str)]) -> (String, String) {
        let mut topology = String::from(
            "name: nobs-t-lab\nmgmt:\n  ipv4-subnet: 203.0.113.0/24\ntopology:\n  nodes:\n",
        );
        let mut vars =
            String::from("prefixes:\n  - prefix: 192.0.2.0/24\n    name: nobs-t-links\nnodes:\n");
        for (at, (name, kind)) in nodes.iter().enumerate() {
            topology.push_str(&format!("    {name}:\n      kind: {kind}\n"));
            vars.push_str(&format!(
                "  {name}:\n    mgmt-ipv4: 203.0.113.{}\n    interfaces:\n      - name: ethernet-1/1\n        ipv4: 192.0.2.{at}/31\n        role: nobs-t-peer\n",
                11 + at
            ));
        }
        (topology, vars)
    }

    fn segment(path: &str) -> &str {
        path.trim_matches('/').rsplit('/').next().unwrap()
    }

    fn fresh_stub() -> Stub {
        fresh_stub_refusing(&[])
    }

    fn fresh_stub_refusing(refused: &[String]) -> Stub {
        let created: Vec<(&str, String)> = Kind::ALL
            .iter()
            .flat_map(|kind| {
                (0..4).map(move |at| {
                    let id = format!("{}-{at}", segment(kind.path()));
                    let body = match kind {
                        Kind::Role => json!({"id": id}),
                        _ => json!({"id": id, "display": id}),
                    };
                    (kind.path(), body.to_string())
                })
            })
            .collect();
        let mut routes: Vec<(&str, &str, u16, &str)> = refused
            .iter()
            .map(|path| ("GET", path.as_str(), 400, NOT_AN_ID))
            .collect();
        routes.push(("GET", "/api/*", 200, NO_RESULTS));
        routes.extend(
            created
                .iter()
                .map(|(path, body)| ("POST", *path, 200, body.as_str())),
        );
        routes.push(("PATCH", "/api/dcim/devices/*", 200, "{}"));
        Stub::start(&routes)
    }

    fn load(stub: &Stub, topology: &str, vars: &str) -> Result<Load, Error> {
        Nautobot::new(stub.url(), TOKEN).load(topology, vars, &mut |_, _, _| {})
    }

    fn calls(stub: &Stub) -> Vec<Call> {
        stub.requests()
            .into_iter()
            .map(|request| {
                let body = if request.body.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&request.body).unwrap()
                };
                (request.method, request.path, body)
            })
            .collect()
    }

    fn device_bodies(stub: &Stub) -> Vec<Value> {
        calls(stub)
            .into_iter()
            .filter(|(method, path, _)| method == "POST" && path == "/api/dcim/devices/")
            .map(|(_, _, body)| body)
            .collect()
    }

    fn get(path: &str) -> Call {
        ("GET".to_string(), path.to_string(), Value::Null)
    }

    fn step(list: &str, filter: &str, body: Value) -> Vec<Call> {
        vec![
            get(&format!("{list}?{filter}")),
            ("POST".to_string(), list.to_string(), body),
        ]
    }

    fn node_calls(name: &str, at: usize, address: &str, mgmt: &str, peers: Value) -> Vec<Call> {
        let device = format!("devices-{at}");
        let interface = format!("interfaces-{}", 2 * at);
        let mgmt_interface = format!("interfaces-{}", 2 * at + 1);
        let ip = format!("ip-addresses-{}", 2 * at);
        let mgmt_ip = format!("ip-addresses-{}", 2 * at + 1);
        let encoded_address = address.replace('/', "%2F");
        let mut all = step(
            "/api/dcim/devices/",
            &format!("name={name}"),
            json!({
                "name": name,
                "role": {"id": "roles-0"},
                "device_type": {"id": "device-types-1"},
                "location": {"id": "locations-0"},
                "status": {"id": "statuses-0"},
                "local_config_context_data": {"observability_intent": {"bgp": {
                    "afi_safi": "ipv4-unicast",
                    "intended_peers": peers,
                }}},
                "custom_fields": {"maintenance": false},
            }),
        );
        all.extend(step(
            "/api/dcim/interfaces/",
            &format!("device={device}&name=ethernet-1%2F1"),
            json!({
                "device": {"id": device},
                "name": "ethernet-1/1",
                "type": "virtual",
                "enabled": true,
                "description": "Interface ethernet-1/1",
                "status": {"id": "statuses-0"},
                "label": "nobs-t-peer",
            }),
        ));
        all.extend(step(
            "/api/ipam/ip-addresses/",
            &format!("address={encoded_address}&namespace=namespaces-0"),
            json!({
                "address": address,
                "status": {"id": "statuses-0"},
                "namespace": {"id": "namespaces-0"},
                "type": "host",
            }),
        ));
        all.extend(step(
            "/api/ipam/ip-address-to-interface/",
            &format!("ip_address={ip}&interface={interface}"),
            json!({"ip_address": {"id": ip}, "interface": {"id": interface}}),
        ));
        all.extend(step(
            "/api/ipam/ip-addresses/",
            &format!("address={mgmt}&namespace=namespaces-0"),
            json!({
                "address": mgmt,
                "status": {"id": "statuses-0"},
                "namespace": {"id": "namespaces-0"},
                "type": "host",
            }),
        ));
        all.extend(step(
            "/api/dcim/interfaces/",
            &format!("device={device}&name=Management0"),
            json!({
                "device": {"id": device},
                "name": "Management0",
                "type": "virtual",
                "enabled": true,
                "description": "Management Interface",
                "status": {"id": "statuses-0"},
                "label": "mgmt",
            }),
        ));
        all.extend(step(
            "/api/ipam/ip-address-to-interface/",
            &format!("ip_address={mgmt_ip}&interface={mgmt_interface}"),
            json!({"ip_address": {"id": mgmt_ip}, "interface": {"id": mgmt_interface}}),
        ));
        all.push((
            "PATCH".to_string(),
            format!("/api/dcim/devices/{device}/"),
            json!({"primary_ip4": {"id": mgmt_ip}}),
        ));
        all
    }

    fn expected_load() -> Vec<Call> {
        let status_types = json!([
            "dcim.device",
            "dcim.interface",
            "dcim.location",
            "ipam.ipaddress",
            "ipam.prefix"
        ]);
        let mut all = step(
            "/api/extras/roles/",
            "name=network_device",
            json!({"name": "network_device", "content_types": ["dcim.device"]}),
        );
        all.extend(step(
            "/api/dcim/manufacturers/",
            "name=Arista",
            json!({"name": "Arista"}),
        ));
        all.extend(step(
            "/api/dcim/device-types/",
            "manufacturer=manufacturers-0&model=cEOS",
            json!({"manufacturer": "Arista", "model": "cEOS"}),
        ));
        all.extend(step(
            "/api/dcim/manufacturers/",
            "name=Nokia",
            json!({"name": "Nokia"}),
        ));
        all.extend(step(
            "/api/dcim/device-types/",
            "manufacturer=manufacturers-1&model=SRLinux",
            json!({"manufacturer": "Nokia", "model": "SRLinux"}),
        ));
        all.extend(step(
            "/api/dcim/location-types/",
            "name=site",
            json!({"name": "site", "content_types": ["dcim.device"]}),
        ));
        all.extend(step(
            "/api/extras/statuses/",
            "name=lab-active",
            json!({"name": "lab-active", "content_types": status_types, "color": "aaf0d1"}),
        ));
        all.extend(step(
            "/api/extras/statuses/",
            "name=Alerted",
            json!({"name": "Alerted", "content_types": status_types, "color": "ff5a36"}),
        ));
        all.extend(step(
            "/api/extras/custom-fields/",
            "limit=200",
            json!({
                "label": "Maintenance",
                "key": "maintenance",
                "type": "boolean",
                "default": false,
                "required": false,
                "content_types": ["dcim.device"],
            }),
        ));
        all.extend(step(
            "/api/dcim/locations/",
            "name=lab",
            json!({
                "name": "lab",
                "location_type": {"id": "location-types-0"},
                "status": {"id": "statuses-0"},
            }),
        ));
        all.extend(step(
            "/api/ipam/namespaces/",
            "name=lab-default",
            json!({"name": "lab-default"}),
        ));
        for (prefix, description) in [
            ("192.0.2.0/24", "nobs-t-links"),
            ("203.0.113.0/24", "lab-mgmt-prefix"),
        ] {
            all.extend(step(
                "/api/ipam/prefixes/",
                &format!(
                    "prefix={}&namespace=namespaces-0",
                    prefix.replace('/', "%2F")
                ),
                json!({
                    "prefix": prefix,
                    "namespace": {"id": "namespaces-0"},
                    "type": "network",
                    "status": {"id": "statuses-0"},
                    "description": description,
                }),
            ));
        }
        all.extend(node_calls(
            "nobs-t-srl1",
            0,
            "192.0.2.0/31",
            "203.0.113.11",
            json!([{"peer_ip": "192.0.2.1", "remote_as": 65002}]),
        ));
        all.extend(node_calls(
            "nobs-t-srl2",
            1,
            "192.0.2.1/31",
            "203.0.113.12",
            json!([{"peer_ip": "192.0.2.0", "remote_as": 65001}]),
        ));
        all
    }

    #[test]
    fn device_type_names_the_manufacturer_and_model_of_each_kind() {
        assert_eq!(device_type("ceos"), Some(("Arista", "cEOS")));
        assert_eq!(device_type("arista_ceos"), Some(("Arista", "cEOS")));
        assert_eq!(device_type("nokia_srlinux"), Some(("Nokia", "SRLinux")));
        assert_eq!(device_type("srl"), Some(("Nokia", "SRLinux")));
        assert_eq!(device_type("linux"), None);
    }

    #[test]
    fn a_bad_input_is_an_input_error_naming_the_cause_before_any_request() {
        let (one, one_vars) = texts(&[("nobs-t-a", "nokia_srlinux")]);
        let (_, two_vars) = texts(&[("nobs-t-a", "nokia_srlinux"), ("nobs-t-b", "nokia_srlinux")]);
        let (two, _) = texts(&[("nobs-t-a", "nokia_srlinux"), ("nobs-t-b", "nokia_srlinux")]);
        let (unknown_kind, unknown_kind_vars) = texts(&[("nobs-t-a", "linux")]);
        let cases = [
            (unknown_kind, unknown_kind_vars, "nobs-t-a: kind linux"),
            (
                one.clone(),
                one_vars.replace("192.0.2.0/31", "192.0.2.300/31"),
                "192.0.2.300/31",
            ),
            (
                one.replace("  ipv4-subnet: 203.0.113.0/24\n", ""),
                one_vars.clone(),
                "mgmt.ipv4-subnet",
            ),
            (one, two_vars, "lab_vars node nobs-t-b"),
            (two, one_vars, "topology node nobs-t-b"),
        ];
        for (topology, vars, needle) in cases {
            let stub = fresh_stub();

            let result = load(&stub, &topology, &vars);

            assert!(
                matches!(&result, Err(Error::Input(reason)) if reason.contains(needle)),
                "{needle}: {result:?}"
            );
            assert!(stub.requests().is_empty(), "{needle}");
        }
    }

    #[test]
    fn load_writes_the_objects_in_order_and_a_second_load_only_updates() {
        let stub = fresh_stub();
        let mut progress = Vec::new();
        let first = Nautobot::new(&format!("{}/", stub.url()), TOKEN)
            .load(&topology(), FIXTURE_VARS, &mut |kind, display, outcome| {
                progress.push((kind.to_string(), display.to_string(), outcome))
            })
            .unwrap();

        assert_eq!(calls(&stub), expected_load());
        let created = [1, 2, 2, 1, 2, 1, 1, 1, 2, 2, 4, 4, 4];
        let words = [
            "role",
            "manufacturer",
            "device type",
            "location type",
            "status",
            "custom field",
            "location",
            "namespace",
            "prefix",
            "device",
            "interface",
            "ip address",
            "ip address to interface",
        ];
        let rows: Vec<(String, usize, usize)> = words
            .iter()
            .zip(created)
            .map(|(word, count)| (word.to_string(), count, 0))
            .collect();
        assert_eq!(first.rows, rows);
        assert_eq!(progress.len(), 27);
        assert_eq!(
            progress[0],
            (
                "role".to_string(),
                "network_device".to_string(),
                Outcome::Created
            )
        );
        assert_eq!(
            progress[1],
            (
                "manufacturer".to_string(),
                "manufacturers-0".to_string(),
                Outcome::Created
            )
        );
        for request in stub.requests() {
            assert_eq!(request.header("Authorization"), Some("Token nobs-t-token"));
            assert_eq!(request.header("Accept"), Some("application/json"));
            assert_eq!(
                request.header("Content-Type"),
                (!request.body.is_empty()).then_some("application/json")
            );
        }

        let found = r#"{"count":1,"next":null,"results":[{"id":"found","display":"found"}]}"#;
        let found_field = r#"{"count":1,"next":null,"results":[{"id":"found","display":"found","key":"maintenance"}]}"#;
        let again = Stub::start(&[
            ("GET", "/api/extras/custom-fields/*", 200, found_field),
            ("GET", "/api/*", 200, found),
            (
                "PATCH",
                "/api/*",
                200,
                r#"{"id":"found","display":"found"}"#,
            ),
        ]);

        let second = load(&again, &topology(), FIXTURE_VARS).unwrap();

        let updated: Vec<(String, usize, usize)> = rows
            .iter()
            .map(|(word, count, _)| (word.clone(), 0, *count))
            .collect();
        assert_eq!(second.rows, updated);
        let made = calls(&again);
        assert!(made.iter().all(|(method, _, _)| method != "POST"));
        assert_eq!(
            made.iter()
                .filter(|(method, _, _)| method == "PATCH")
                .count(),
            29
        );
        let devices: Vec<&Value> = made
            .iter()
            .map(|(_, _, body)| body)
            .filter(|body| body.get("local_config_context_data").is_some())
            .collect();
        assert_eq!(devices.len(), 2);
        assert!(devices
            .iter()
            .all(|body| body.get("custom_fields").is_none()));
    }

    #[test]
    fn every_device_gets_the_device_type_of_its_own_kind() {
        let stub = fresh_stub();
        let (topology, vars) = texts(&[("nobs-t-eos", "ceos"), ("nobs-t-srl", "nokia_srlinux")]);

        load(&stub, &topology, &vars).unwrap();

        let types: Vec<Value> = device_bodies(&stub)
            .iter()
            .map(|body| body["device_type"]["id"].clone())
            .collect();
        assert_eq!(types, [json!("device-types-0"), json!("device-types-1")]);
    }

    #[test]
    fn a_status_that_is_not_2xx_stops_the_load_with_the_method_path_status_and_first_line() {
        let stub = Stub::start(&[
            ("GET", "/api/*", 200, NO_RESULTS),
            (
                "POST",
                "/api/extras/roles/",
                200,
                r#"{"id":"r1","display":"r1"}"#,
            ),
            (
                "POST",
                "/api/dcim/manufacturers/",
                400,
                "no good\nsecond line",
            ),
        ]);

        let error = load(&stub, &topology(), FIXTURE_VARS).unwrap_err();

        assert!(
            matches!(&error, Error::Http { status: 400, .. }),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "POST /api/dcim/manufacturers/ failed: 400 no good"
        );
        assert_eq!(stub.requests().len(), 4);
    }

    #[test]
    fn delete_reads_every_page_and_sends_one_bulk_delete_per_kind_that_found_something() {
        let page = |ids: &[&str]| {
            let items: Vec<String> = ids.iter().map(|id| format!(r#"{{"id":"{id}"}}"#)).collect();
            format!(r#"{{"next":null,"results":[{}]}}"#, items.join(","))
        };
        let mapping = |id: &str| {
            format!(r#"{{"next":null,"results":[{{"id":"{id}","ip_address":{{"id":"ip-1"}}}}]}}"#)
        };
        let first_devices = r#"{"next":"http://nautobot.internal:8080/api/dcim/devices/?location=loc-1&limit=200&offset=1","results":[{"id":"dev-1"}]}"#;
        let fields = r#"{"next":null,"results":[{"id":"cf-0","key":"other"},{"id":"cf-1","key":"maintenance"}]}"#;
        let gets = [
            ("/api/dcim/locations/?name=lab&limit=200", page(&["loc-1"])),
            (
                "/api/ipam/namespaces/?name=lab-default&limit=200",
                page(&["ns-1"]),
            ),
            (
                "/api/dcim/devices/?location=loc-1&limit=200",
                first_devices.to_string(),
            ),
            (
                "/api/dcim/devices/?location=loc-1&limit=200&offset=1",
                page(&["dev-2"]),
            ),
            (
                "/api/dcim/interfaces/?device=dev-1&limit=200",
                page(&["if-1", "if-2"]),
            ),
            (
                "/api/dcim/interfaces/?device=dev-2&limit=200",
                page(&["if-3"]),
            ),
            (
                "/api/ipam/ip-address-to-interface/?interface=if-1&limit=200",
                mapping("map-1"),
            ),
            (
                "/api/ipam/ip-address-to-interface/?interface=if-2&limit=200",
                mapping("map-2"),
            ),
            (
                "/api/ipam/prefixes/?namespace=ns-1&limit=200",
                page(&["pre-1"]),
            ),
            (
                "/api/ipam/ip-addresses/?namespace=ns-1&limit=200",
                page(&["ip-1"]),
            ),
            (
                "/api/extras/statuses/?name=lab-active&limit=200",
                page(&["st-1"]),
            ),
            (
                "/api/extras/statuses/?name=Alerted&limit=200",
                page(&["st-2"]),
            ),
            ("/api/extras/custom-fields/?limit=200", fields.to_string()),
            (
                "/api/dcim/manufacturers/?name=Arista&limit=200",
                page(&["mf-0"]),
            ),
            (
                "/api/dcim/device-types/?manufacturer=mf-0&model=cEOS&limit=200",
                page(&["dt-1"]),
            ),
            (
                "/api/dcim/manufacturers/?name=Nokia&limit=200",
                page(&["mf-1"]),
            ),
            (
                "/api/dcim/device-types/?manufacturer=mf-1&model=SRLinux&limit=200",
                page(&["dt-2"]),
            ),
            (
                "/api/extras/roles/?name=network_device&limit=200",
                page(&["role-1"]),
            ),
        ];
        let mut routes: Vec<(&str, &str, u16, &str)> = gets
            .iter()
            .map(|(path, body)| ("GET", *path, 200, body.as_str()))
            .collect();
        routes.push(("GET", "/api/*", 200, r#"{"next":null,"results":[]}"#));
        routes.push(("DELETE", "/api/*", 204, ""));
        let stub = Stub::start(&routes);
        let mut progress = Vec::new();

        let deleted = Nautobot::new(stub.url(), TOKEN)
            .delete(&mut |kind, count| progress.push((kind.to_string(), count)))
            .unwrap();

        let rows = [
            ("ip address to interface", 2),
            ("ip address", 1),
            ("device", 2),
            ("prefix", 1),
            ("namespace", 1),
            ("location", 1),
            ("location type", 0),
            ("status", 2),
            ("custom field", 1),
            ("device type", 2),
            ("manufacturer", 2),
            ("role", 1),
        ]
        .map(|(kind, count)| (kind.to_string(), count));
        assert_eq!(deleted.rows, rows);
        assert_eq!(progress, rows);
        let sent: Vec<(String, Value)> = calls(&stub)
            .into_iter()
            .filter(|(method, _, _)| method == "DELETE")
            .map(|(_, path, body)| (path, body))
            .collect();
        let bulk = |path: &str, ids: &[&str]| {
            (
                path.to_string(),
                Value::Array(ids.iter().map(|id| json!({"id": id})).collect()),
            )
        };
        assert_eq!(
            sent,
            [
                bulk("/api/ipam/ip-address-to-interface/", &["map-1", "map-2"]),
                bulk("/api/ipam/ip-addresses/", &["ip-1"]),
                bulk("/api/dcim/devices/", &["dev-1", "dev-2"]),
                bulk("/api/ipam/prefixes/", &["pre-1"]),
                bulk("/api/ipam/namespaces/", &["ns-1"]),
                bulk("/api/dcim/locations/", &["loc-1"]),
                bulk("/api/extras/statuses/", &["st-1", "st-2"]),
                bulk("/api/extras/custom-fields/", &["cf-1"]),
                bulk("/api/dcim/device-types/", &["dt-1", "dt-2"]),
                bulk("/api/dcim/manufacturers/", &["mf-0", "mf-1"]),
                bulk("/api/extras/roles/", &["role-1"]),
            ]
        );
    }

    #[test]
    fn devices_are_written_in_the_order_of_the_topology_file() {
        let stub = fresh_stub();
        let (topology, vars) = texts(&[
            ("nobs-t-zulu", "nokia_srlinux"),
            ("nobs-t-alpha", "nokia_srlinux"),
            ("nobs-t-mike", "nokia_srlinux"),
        ]);

        load(&stub, &topology, &vars).unwrap();

        let names: Vec<Value> = device_bodies(&stub)
            .iter()
            .map(|body| body["name"].clone())
            .collect();
        assert_eq!(
            names,
            [
                json!("nobs-t-zulu"),
                json!("nobs-t-alpha"),
                json!("nobs-t-mike")
            ]
        );
    }

    #[test]
    fn more_than_one_match_for_a_natural_key_is_an_input_error_and_writes_nothing() {
        let stub = Stub::start(&[(
            "GET",
            "/api/*",
            200,
            r#"{"count":2,"next":null,"results":[{"id":"a"},{"id":"b"}]}"#,
        )]);

        let result = load(&stub, &topology(), FIXTURE_VARS);

        assert!(
            matches!(&result, Err(Error::Input(reason)) if reason == "role network_device: 2 objects match in Nautobot"),
            "{result:?}"
        );
        assert_eq!(stub.requests().len(), 1);
    }

    #[test]
    fn the_custom_field_is_found_by_its_key_among_the_other_custom_fields() {
        let fields = r#"{"next":null,"results":[{"id":"cf-other","key":"other"},{"id":"cf-main","key":"maintenance"}]}"#;
        let stub = Stub::start(&[
            ("GET", "/api/extras/custom-fields/?limit=200", 200, fields),
            ("GET", "/api/*", 200, NO_RESULTS),
            ("POST", "/api/*", 200, r#"{"id":"made","display":"made"}"#),
            (
                "PATCH",
                "/api/*",
                200,
                r#"{"id":"cf-main","display":"cf-main"}"#,
            ),
        ]);
        let (topology, vars) = texts(&[("nobs-t-a", "nokia_srlinux")]);

        let loaded = load(&stub, &topology, &vars).unwrap();

        let made = calls(&stub);
        let field_writes: Vec<(&str, &str)> = made
            .iter()
            .filter(|(method, path, _)| method != "GET" && path.starts_with("/api/extras/custom-"))
            .map(|(method, path, _)| (method.as_str(), path.as_str()))
            .collect();
        assert_eq!(
            field_writes,
            [("PATCH", "/api/extras/custom-fields/cf-main/")]
        );
        assert_eq!(loaded.rows[5], ("custom field".to_string(), 0, 1));
    }

    #[test]
    fn delete_on_an_empty_nautobot_sends_no_filter_on_a_parent_and_no_delete() {
        let refused = r#"{"location":["Select a valid choice."]}"#;
        let stub = Stub::start(&[
            ("GET", "/api/dcim/devices/?location=*", 400, refused),
            ("GET", "/api/ipam/prefixes/?namespace=*", 400, refused),
            ("GET", "/api/*", 200, r#"{"next":null,"results":[]}"#),
            ("DELETE", "/api/*", 204, ""),
        ]);

        let deleted = Nautobot::new(stub.url(), TOKEN)
            .delete(&mut |_, _| {})
            .unwrap();

        let words = [
            "ip address to interface",
            "ip address",
            "device",
            "prefix",
            "namespace",
            "location",
            "location type",
            "status",
            "custom field",
            "device type",
            "manufacturer",
            "role",
        ];
        let rows: Vec<(String, usize)> = words.iter().map(|word| (word.to_string(), 0)).collect();
        assert_eq!(deleted.rows, rows);
        for (method, path, _) in calls(&stub) {
            assert_eq!(method, "GET", "{path}");
            assert!(
                !path.contains("location=") && !path.contains("namespace="),
                "{path}"
            );
        }
    }

    fn names_in_related_filters() -> Vec<String> {
        let mut paths: Vec<String> = [
            "/api/dcim/device-types/?manufacturer=Arista*",
            "/api/dcim/device-types/?manufacturer=Nokia*",
            "/api/dcim/devices/?location=lab*",
            "/api/dcim/interfaces/?device=nobs-t-*",
            "/api/ipam/prefixes/?namespace=lab-default*",
            "/api/ipam/ip-addresses/?namespace=lab-default*",
            "/api/ipam/ip-address-to-interface/?ip_address=192*",
            "/api/ipam/ip-address-to-interface/?ip_address=203*",
            "/api/ipam/ip-address-to-interface/?interface=ethernet*",
            "/api/ipam/ip-address-to-interface/?interface=Management0*",
        ]
        .map(String::from)
        .to_vec();
        for prefix in ["192.0.2.0%2F24", "203.0.113.0%2F24"] {
            paths.push(format!(
                "/api/ipam/prefixes/?prefix={prefix}&namespace=lab-default"
            ));
        }
        for address in [
            "192.0.2.0%2F31",
            "192.0.2.1%2F31",
            "203.0.113.11",
            "203.0.113.12",
        ] {
            paths.push(format!(
                "/api/ipam/ip-addresses/?address={address}&namespace=lab-default"
            ));
        }
        for ip in 0..4 {
            for interface in ["ethernet-1%2F1", "Management0"] {
                paths.push(format!(
                    "/api/ipam/ip-address-to-interface/?ip_address=ip-addresses-{ip}&interface={interface}"
                ));
            }
        }
        paths
    }

    #[test]
    fn a_load_sends_a_related_object_filter_only_with_the_id_of_an_object_it_has_written() {
        let stub = fresh_stub_refusing(&names_in_related_filters());

        let loaded = load(&stub, &topology(), FIXTURE_VARS);

        assert!(loaded.is_ok(), "{loaded:?}");
    }

    #[test]
    fn delete_on_an_empty_nautobot_twice_sends_no_related_object_filter_and_no_delete() {
        let stub = fresh_stub_refusing(&names_in_related_filters());
        let words = [
            "ip address to interface",
            "ip address",
            "device",
            "prefix",
            "namespace",
            "location",
            "location type",
            "status",
            "custom field",
            "device type",
            "manufacturer",
            "role",
        ];
        let rows: Vec<(String, usize)> = words.iter().map(|word| (word.to_string(), 0)).collect();

        for _ in 0..2 {
            let deleted = Nautobot::new(stub.url(), TOKEN)
                .delete(&mut |_, _| {})
                .unwrap();

            assert_eq!(deleted.rows, rows);
        }
        assert!(calls(&stub).iter().all(|(method, _, _)| method == "GET"));
    }

    fn lab_found(deletes: &[(&str, &str, u16, &str)]) -> Stub {
        let page = |items: &str| format!(r#"{{"next":null,"results":[{items}]}}"#);
        let locations = page(r#"{"id":"loc-1"}"#);
        let namespaces = page(r#"{"id":"ns-1"}"#);
        let devices = page(r#"{"id":"dev-1"}"#);
        let interfaces = page(r#"{"id":"if-1"}"#);
        let mappings = page(r#"{"id":"map-1","ip_address":{"id":"ip-1"}}"#);
        let addresses = page(r#"{"id":"ip-1"},{"id":"ip-2"}"#);
        let empty = page("");
        let mut routes: Vec<(&str, &str, u16, &str)> = vec![
            ("GET", "/api/dcim/locations/*", 200, &locations),
            ("GET", "/api/ipam/namespaces/*", 200, &namespaces),
            ("GET", "/api/dcim/devices/*", 200, &devices),
            ("GET", "/api/dcim/interfaces/*", 200, &interfaces),
            ("GET", "/api/ipam/ip-address-to-interface/*", 200, &mappings),
            ("GET", "/api/ipam/ip-addresses/*", 200, &addresses),
            ("GET", "/api/*", 200, &empty),
        ];
        routes.extend_from_slice(deletes);
        routes.push(("DELETE", "/api/*", 204, ""));
        Stub::start(&routes)
    }

    fn deletes(stub: &Stub) -> Vec<(String, Value)> {
        calls(stub)
            .into_iter()
            .filter(|(method, _, _)| method == "DELETE")
            .map(|(_, path, body)| (path, body))
            .collect()
    }

    #[test]
    fn delete_removes_every_address_in_the_namespace_even_one_no_mapping_names() {
        let stub = lab_found(&[]);

        Nautobot::new(stub.url(), TOKEN)
            .delete(&mut |_, _| {})
            .unwrap();

        let sent = deletes(&stub);
        let addresses = sent
            .iter()
            .find(|(path, _)| path == "/api/ipam/ip-addresses/")
            .unwrap();
        assert_eq!(addresses.1, json!([{"id": "ip-1"}, {"id": "ip-2"}]));
    }

    #[test]
    fn a_delete_that_is_refused_stops_the_delete_with_the_method_path_status_and_first_line() {
        let stub = lab_found(&[(
            "DELETE",
            "/api/ipam/ip-addresses/",
            500,
            "cannot delete\nsecond line",
        )]);

        let error = Nautobot::new(stub.url(), TOKEN)
            .delete(&mut |_, _| {})
            .unwrap_err();

        assert!(
            matches!(&error, Error::Http { method, path, status: 500, .. }
                if method == "DELETE" && path == "/api/ipam/ip-addresses/"),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "DELETE /api/ipam/ip-addresses/ failed: 500 cannot delete"
        );
        let paths: Vec<String> = deletes(&stub).into_iter().map(|(path, _)| path).collect();
        assert_eq!(
            paths,
            [
                "/api/ipam/ip-address-to-interface/",
                "/api/ipam/ip-addresses/"
            ]
        );
    }
}
