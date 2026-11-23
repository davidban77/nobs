use std::path::Path;

use crate::render::{Renderer, TEMPLATES};
use crate::sot::{Error, SourceOfTruth};

pub fn intended(
    lab: &Path,
    name: &str,
    kind: &str,
    source: &dyn SourceOfTruth,
) -> Result<String, String> {
    let renderer = Renderer::new(&lab.join(TEMPLATES));
    let file = renderer.template(kind)?;
    let vars = source
        .node_vars(name)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            Error::NotFound {
                kind: "device".to_string(),
                device: name.to_string(),
            }
            .to_string()
        })?;
    renderer.config(&file, name, kind, &vars)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::infrahub::Infrahub;
    use crate::lab_vars;
    use crate::sot::Fixed;

    const LAB_VARS: &str =
        include_str!("../../tests/fixtures/project-two-node/expected/lab_vars.yml");
    const SRL1: &str =
        include_str!("../../tests/fixtures/project-two-node/expected/configs/nobs-t-srl1.cfg");
    const SRL2: &str =
        include_str!("../../tests/fixtures/project-two-node/expected/configs/nobs-t-srl2.cfg");
    const SRL_TEMPLATE: &str =
        include_str!("../../tests/fixtures/project-two-node/templates/srl.j2");

    struct Lab(PathBuf);

    impl Lab {
        fn new(tag: &str, template: Option<&str>) -> Lab {
            let root =
                std::env::temp_dir().join(format!("nobs-intended-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join(TEMPLATES)).unwrap();
            if let Some(template) = template {
                fs::write(root.join(TEMPLATES).join("srl.j2"), template).unwrap();
            }
            Lab(root)
        }
    }

    impl Drop for Lab {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixed() -> Fixed {
        Fixed::from_lab_vars(&lab_vars::read(LAB_VARS).unwrap())
    }

    #[test]
    fn each_node_of_the_project_fixture_renders_its_startup_config_through_the_source() {
        let lab = Lab::new("renders", Some(SRL_TEMPLATE));
        let source = fixed();
        assert_eq!(
            intended(&lab.0, "nobs-t-srl1", "nokia_srlinux", &source),
            Ok(SRL1.to_string())
        );
        assert_eq!(
            intended(&lab.0, "nobs-t-srl2", "nokia_srlinux", &source),
            Ok(SRL2.to_string())
        );
    }

    #[test]
    fn a_node_the_source_does_not_know_is_not_found() {
        let lab = Lab::new("unknown", Some(SRL_TEMPLATE));
        assert_eq!(
            intended(&lab.0, "nobs-t-srl9", "nokia_srlinux", &fixed()),
            Err("device nobs-t-srl9 not found in the source of truth.".to_string())
        );
    }

    #[test]
    fn infrahub_gives_no_data_for_templates() {
        let lab = Lab::new("infrahub", Some(SRL_TEMPLATE));
        let source = Infrahub::new("http://nobs-t-infrahub:8000", "");
        assert_eq!(
            intended(&lab.0, "nobs-t-srl1", "nokia_srlinux", &source),
            Err("the source of truth gives no data for templates".to_string())
        );
    }

    #[test]
    fn a_kind_without_a_template_is_said_before_the_source_is_asked() {
        let lab = Lab::new("no-template", None);
        let expected =
            Err("no template for kind nokia_srlinux: expected templates/srl.j2".to_string());
        assert_eq!(
            intended(&lab.0, "nobs-t-srl9", "nokia_srlinux", &fixed()),
            expected
        );
        let infrahub = Infrahub::new("http://nobs-t-infrahub:8000", "");
        assert_eq!(
            intended(&lab.0, "nobs-t-srl1", "nokia_srlinux", &infrahub),
            expected
        );
    }
}
