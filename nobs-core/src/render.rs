use std::path::{Path, PathBuf};

use minijinja::syntax::SyntaxConfig;
use minijinja::value::Serde;
use minijinja::{path_loader, Environment, UndefinedBehavior};

pub struct Renderer {
    folder: PathBuf,
    environment: Environment<'static>,
}

impl Renderer {
    pub fn new(folder: &Path) -> Renderer {
        let mut environment = Environment::new();
        environment.set_loader(path_loader(folder));
        environment.set_undefined_behavior(UndefinedBehavior::Strict);
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

    fn render_from_source(
        tag: &str,
        source: &str,
        values: &serde_json::Value,
    ) -> Result<String, String> {
        let folder = std::env::temp_dir().join(format!("nobs-render-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("motd.j2"), source).unwrap();
        let result = Renderer::new(&folder).render("motd.j2", values);
        std::fs::remove_dir_all(&folder).unwrap();
        result
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
