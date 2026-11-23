use std::path::{Path, PathBuf};

use minijinja::syntax::SyntaxConfig;
use minijinja::value::Serde;
use minijinja::{path_loader, Environment, UndefinedBehavior};
use serde_json::{json, Map, Value};

pub const TEMPLATES: &str = "templates";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NodeVars {
    pub entry: Map<String, Value>,
    pub peers: Option<Value>,
}

impl NodeVars {
    pub fn values(&self, name: &str, kind: &str) -> Value {
        let mut data: Map<String, Value> = self
            .entry
            .iter()
            .filter(|(key, _)| *key != "mgmt-ipv4")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        data.insert("name".to_string(), name.into());
        data.insert("kind".to_string(), kind.into());
        if let Some(address) = self.entry.get("mgmt-ipv4") {
            data.insert("mgmt_ipv4".to_string(), address.clone());
        }
        data.entry("interfaces").or_insert_with(|| json!([]));
        let sessions = self.peers.clone().unwrap_or_else(|| json!([]));
        data.insert("sessions".to_string(), sessions);
        Value::Object(data)
    }
}

pub struct Renderer {
    folder: PathBuf,
    environment: Environment<'static>,
}

impl Renderer {
    pub fn new(folder: &Path) -> Renderer {
        let mut environment = Environment::new();
        environment.set_loader(path_loader(folder));
        environment.set_undefined_behavior(UndefinedBehavior::Strict);
        environment.set_debug(true);
        let syntax = SyntaxConfig::builder()
            .keep_trailing_newline(true)
            .trim_blocks(true)
            .lstrip_blocks(true)
            .build()
            .expect("the default delimiters are valid");
        environment.set_syntax(syntax);
        Renderer {
            folder: folder.to_path_buf(),
            environment,
        }
    }

    pub fn exists(&self, template: &str) -> bool {
        self.folder.join(template).is_file()
    }

    pub fn render(&self, template: &str, values: &serde_json::Value) -> Result<String, String> {
        let template = self
            .environment
            .get_template(template)
            .map_err(|error| error.to_string())?;
        template
            .render(Serde(values))
            .map_err(|error| error.to_string())
    }

    pub fn template(&self, kind: &str) -> Result<String, String> {
        let file = template_name(kind);
        if !self.exists(&file) {
            return Err(format!(
                "no template for kind {kind}: expected {TEMPLATES}/{file}"
            ));
        }
        Ok(file)
    }

    pub fn config(
        &self,
        file: &str,
        name: &str,
        kind: &str,
        vars: &NodeVars,
    ) -> Result<String, String> {
        self.render(file, &vars.values(name, kind))
            .map_err(|error| format!("{TEMPLATES}/{file}: {error}"))
    }
}

pub fn template_name(kind: &str) -> String {
    let short = match kind {
        "nokia_srlinux" | "srl" => "srl",
        "ceos" | "arista_ceos" => "ceos",
        other => other,
    };
    format!("{short}.j2")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn with_templates<T>(tag: &str, files: &[(&str, &str)], run: impl FnOnce(&Renderer) -> T) -> T {
        let folder = std::env::temp_dir().join(format!("nobs-render-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        for (file, source) in files {
            std::fs::write(folder.join(file), source).unwrap();
        }
        let result = run(&Renderer::new(&folder));
        std::fs::remove_dir_all(&folder).unwrap();
        result
    }

    fn render_from_source(
        tag: &str,
        source: &str,
        values: &serde_json::Value,
    ) -> Result<String, String> {
        with_templates(tag, &[("motd.j2", source)], |renderer| {
            renderer.render("motd.j2", values)
        })
    }

    fn node_vars(entry: Value, peers: Option<Value>) -> NodeVars {
        let Value::Object(entry) = entry else {
            panic!("the entry is not a map");
        };
        NodeVars { entry, peers }
    }

    fn config_from_source(tag: &str, source: &str, vars: &NodeVars) -> Result<String, String> {
        with_templates(tag, &[("motd.j2", source)], |renderer| {
            renderer.config("motd.j2", "nobs-t-srl1", "nokia_srlinux", vars)
        })
    }

    #[test]
    fn template_maps_each_kind_to_its_file_and_a_missing_file_gives_the_kind_and_the_path() {
        with_templates(
            "kinds",
            &[("srl.j2", ""), ("ceos.j2", ""), ("linux.j2", "")],
            |renderer| {
                assert_eq!(renderer.template("nokia_srlinux"), Ok("srl.j2".to_string()));
                assert_eq!(renderer.template("srl"), Ok("srl.j2".to_string()));
                assert_eq!(renderer.template("arista_ceos"), Ok("ceos.j2".to_string()));
                assert_eq!(renderer.template("linux"), Ok("linux.j2".to_string()));
                assert_eq!(
                    renderer.template("nobs_t_kind"),
                    Err(
                        "no template for kind nobs_t_kind: expected templates/nobs_t_kind.j2"
                            .to_string()
                    )
                );
            },
        );
    }

    #[test]
    fn values_give_every_address_the_management_address_and_every_other_key() {
        let vars = node_vars(
            json!({
                "mgmt-ipv4": "203.0.113.11",
                "nobs-t-code": "65001",
                "interfaces": [{"name": "ethernet-1/1", "ipv4": ["192.0.2.0/31", "198.51.100.0/31"]}],
            }),
            Some(json!([{"peer_ip": "192.0.2.1", "remote_as": 65002}])),
        );
        assert_eq!(
            vars.values("nobs-t-srl1", "nokia_srlinux"),
            json!({
                "name": "nobs-t-srl1",
                "kind": "nokia_srlinux",
                "mgmt_ipv4": "203.0.113.11",
                "nobs-t-code": "65001",
                "interfaces": [{"name": "ethernet-1/1", "ipv4": ["192.0.2.0/31", "198.51.100.0/31"]}],
                "sessions": [{"peer_ip": "192.0.2.1", "remote_as": 65002}],
            })
        );
        assert_eq!(
            NodeVars::default().values("a", "linux"),
            json!({"name": "a", "kind": "linux", "interfaces": [], "sessions": []})
        );
    }

    #[test]
    fn config_renders_the_values_with_every_address_of_a_list() {
        let vars = node_vars(
            json!({
                "mgmt-ipv4": "203.0.113.11",
                "interfaces": [{"name": "ethernet-1/1", "ipv4": ["192.0.2.0/31", "198.51.100.0/31"]}],
            }),
            None,
        );
        let text = config_from_source(
            "addresses",
            "{{ name }} {{ kind }} {{ mgmt_ipv4 }}\n{% for interface in interfaces %}{% for address in interface.ipv4 %}{{ interface.name }} {{ address }}\n{% endfor %}{% endfor %}",
            &vars,
        );
        assert_eq!(
            text,
            Ok("nobs-t-srl1 nokia_srlinux 203.0.113.11\nethernet-1/1 192.0.2.0/31\nethernet-1/1 198.51.100.0/31\n".to_string())
        );
    }

    #[test]
    fn a_name_the_data_lacks_is_an_error_naming_the_template_and_the_name() {
        let error =
            config_from_source("missing", "{{ hostname }}\n", &NodeVars::default()).unwrap_err();
        assert!(error.starts_with("templates/motd.j2: "), "{error}");
        assert!(error.contains("hostname"), "{error}");
    }

    #[test]
    fn an_attribute_the_data_lacks_is_an_error_naming_the_template_and_the_attribute() {
        let vars = node_vars(json!({"interfaces": [{"name": "ethernet-1/1"}]}), None);
        let error = config_from_source(
            "attribute",
            "{% for interface in interfaces %}{{ interface.mtu }}{% endfor %}\n",
            &vars,
        )
        .unwrap_err();
        assert!(error.starts_with("templates/motd.j2: "), "{error}");
        assert!(error.contains("mtu"), "{error}");
    }

    #[test]
    fn an_error_is_one_line_without_the_template_source_or_the_data() {
        let vars = node_vars(json!({"nobs-t-key": "nobs-t-data-value"}), None);
        for (tag, source) in [
            ("syntax", "nobs-t-source-line {{ name }}\n{% if %}\n"),
            (
                "undefined",
                "nobs-t-source-line {{ name }}\n{{ hostname }}\n",
            ),
        ] {
            let error = config_from_source(tag, source, &vars).unwrap_err();
            assert!(error.starts_with("templates/motd.j2: "), "{error}");
            assert!(error.contains("motd.j2:2"), "{error}");
            assert_eq!(error.lines().count(), 1, "{error}");
            for leak in ["nobs-t-source-line", "nobs-t-data-value", "nobs-t-srl1"] {
                assert!(!error.contains(leak), "{error}");
            }
        }
    }

    #[test]
    fn a_variable_the_template_names_and_the_data_lacks_is_an_error_naming_both() {
        let error = render_from_source(
            "undefined",
            "host {{ name }}\ncontact {{ hostname }}\n",
            &json!({"name": "nobs-t-srl1"}),
        )
        .unwrap_err();
        assert!(error.contains("hostname"), "{error}");
        assert!(error.contains("motd.j2"), "{error}");
    }

    #[test]
    fn template_name_maps_the_kinds() {
        assert_eq!(template_name("nokia_srlinux"), "srl.j2");
        assert_eq!(template_name("srl"), "srl.j2");
        assert_eq!(template_name("ceos"), "ceos.j2");
        assert_eq!(template_name("arista_ceos"), "ceos.j2");
        assert_eq!(template_name("other"), "other.j2");
    }

    #[test]
    fn the_trailing_newline_of_a_template_is_kept() {
        let text =
            render_from_source("newline", "host {{ name }}\n", &json!({"name": "a"})).unwrap();
        assert_eq!(text, "host a\n");
    }
}
