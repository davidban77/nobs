use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml::de::{DeTable, Deserializer, Error};
use toml::Spanned;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workshop {
    pub name: String,
    pub title: String,
    pub project: String,
    pub network: String,
    pub workflow_label: String,
    pub cascade_prefix: String,
    pub scenario_prefix: String,
    pub evidence_query: String,
    pub silence_hints: Vec<String>,
    pub schema_file: PathBuf,
    #[serde(default)]
    pub image_pins: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lab {
    pub topology: Option<PathBuf>,
    pub compose: Option<PathBuf>,
    pub project: Option<String>,
    pub network: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Settings {
    Workshop(Workshop),
    Lab(Lab),
}

pub fn read_settings(text: &str) -> Result<Settings, String> {
    let mut root = DeTable::parse(text).map_err(|error| at_line(text, &error))?;
    let kind = root
        .get_mut()
        .remove("kind")
        .ok_or_else(|| "kind: missing".to_string())?;
    match kind.get_ref().as_str() {
        Some("workshop") => deserialize(text, root).map(Settings::Workshop),
        Some("lab") => deserialize(text, root).map(Settings::Lab),
        _ => Err("kind: must be \"workshop\" or \"lab\"".to_string()),
    }
}

pub fn read_settings_file(path: &Path) -> Result<Settings, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    read_settings(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn deserialize<'i, T: Deserialize<'i>>(
    text: &str,
    root: Spanned<DeTable<'i>>,
) -> Result<T, String> {
    T::deserialize(Deserializer::from(root.clone())).map_err(|error| {
        let message = error.message();
        if let Some(key) = quoted(message, "missing field `") {
            return format!("{key}: missing");
        }
        if let Some(key) = quoted(message, "unknown field `") {
            return format!("{key}: unknown key");
        }
        match error
            .span()
            .and_then(|span| key_at(root.get_ref(), span.start))
        {
            Some(key) => format!("{key}: {message}"),
            None => at_line(text, &error),
        }
    })
}

fn quoted<'m>(message: &'m str, prefix: &str) -> Option<&'m str> {
    message.strip_prefix(prefix)?.split('`').next()
}

fn key_at(table: &DeTable, offset: usize) -> Option<String> {
    table.iter().find_map(|(key, value)| {
        if let Some(inner) = value
            .get_ref()
            .as_table()
            .and_then(|table| key_at(table, offset))
        {
            return Some(format!("{}.{inner}", key.get_ref()));
        }
        (key.span().contains(&offset) || value.span().contains(&offset))
            .then(|| key.get_ref().to_string())
    })
}

fn at_line(text: &str, error: &Error) -> String {
    match error.span() {
        Some(span) => {
            let before = &text.as_bytes()[..span.start.min(text.len())];
            let line = before.iter().filter(|byte| **byte == b'\n').count() + 1;
            format!("line {line}: {}", error.message())
        }
        None => error.message().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSHOP: &str = r#"kind = "workshop"
name = "nobs-t-one"
title = "Workshop one"
project = "nobs-t-one"
network = "workshop-nobs-t-one"
workflow_label = "nobs_t_one_quarantine_bgp"
cascade_prefix = "nobs-t-one-cascade-"
scenario_prefix = "nobs-t-one-"
evidence_query = "{device=\"srl1\"}"
silence_hints = ["Workshop", "workshop"]
schema_file = "infrahub/schema.yml"

[image_pins]
sonda = "ghcr.io/davidban77/sonda:1.22.2"
"#;

    fn workshop(text: &str) -> Workshop {
        match read_settings(text) {
            Ok(Settings::Workshop(workshop)) => workshop,
            other => panic!("{other:?}"),
        }
    }

    fn without(line: &str) -> String {
        WORKSHOP.replace(&format!("{line}\n"), "")
    }

    #[test]
    fn the_full_workshop_text_parses_to_the_eleven_settings() {
        assert_eq!(
            workshop(WORKSHOP),
            Workshop {
                name: "nobs-t-one".to_string(),
                title: "Workshop one".to_string(),
                project: "nobs-t-one".to_string(),
                network: "workshop-nobs-t-one".to_string(),
                workflow_label: "nobs_t_one_quarantine_bgp".to_string(),
                cascade_prefix: "nobs-t-one-cascade-".to_string(),
                scenario_prefix: "nobs-t-one-".to_string(),
                evidence_query: "{device=\"srl1\"}".to_string(),
                silence_hints: vec!["Workshop".to_string(), "workshop".to_string()],
                schema_file: PathBuf::from("infrahub/schema.yml"),
                image_pins: BTreeMap::from([(
                    "sonda".to_string(),
                    "ghcr.io/davidban77/sonda:1.22.2".to_string()
                )]),
            }
        );
    }

    #[test]
    fn a_missing_title_is_an_error_naming_title() {
        assert_eq!(
            read_settings(&without("title = \"Workshop one\"")),
            Err("title: missing".to_string())
        );
    }

    #[test]
    fn an_unknown_key_is_an_error_naming_it() {
        let text = WORKSHOP.replace(
            "kind = \"workshop\"\n",
            "kind = \"workshop\"\ncolour = \"red\"\n",
        );
        assert_eq!(read_settings(&text), Err("colour: unknown key".to_string()));
        assert_eq!(
            read_settings("kind = \"lab\"\ncolour = \"red\"\n"),
            Err("colour: unknown key".to_string())
        );
    }

    #[test]
    fn silence_hints_that_is_not_a_list_is_an_error() {
        let text = WORKSHOP.replace(
            "silence_hints = [\"Workshop\", \"workshop\"]",
            "silence_hints = \"x\"",
        );
        let error = read_settings(&text).unwrap_err();
        assert!(error.starts_with("silence_hints: "), "{error}");
    }

    #[test]
    fn a_wrong_type_error_names_the_key_on_one_line() {
        let text = WORKSHOP.replace("sonda = \"ghcr.io/davidban77/sonda:1.22.2\"", "sonda = 1");
        assert_eq!(
            read_settings(&text),
            Err("image_pins.sonda: invalid type: integer `1`, expected a string".to_string())
        );
    }

    #[test]
    fn a_syntax_error_names_the_line_on_one_line() {
        let error = read_settings("kind = \"lab\"\nproject = \n").unwrap_err();
        assert!(error.starts_with("line 2: "), "{error}");
        assert!(!error.contains('\n'), "{error}");
    }

    #[test]
    fn a_lab_with_only_project_leaves_the_other_three_none() {
        assert_eq!(
            read_settings("kind = \"lab\"\nproject = \"nobs-t-yaml\"\n"),
            Ok(Settings::Lab(Lab {
                project: Some("nobs-t-yaml".to_string()),
                ..Lab::default()
            }))
        );
    }

    #[test]
    fn another_kind_is_an_error() {
        assert_eq!(
            read_settings(&WORKSHOP.replace("kind = \"workshop\"", "kind = \"other\"")),
            Err("kind: must be \"workshop\" or \"lab\"".to_string())
        );
    }

    #[test]
    fn a_missing_kind_is_an_error() {
        assert_eq!(
            read_settings(&without("kind = \"workshop\"")),
            Err("kind: missing".to_string())
        );
    }

    #[test]
    fn an_empty_image_pins_is_fine_and_a_missing_one_is_an_empty_map() {
        let empty = WORKSHOP.replace("sonda = \"ghcr.io/davidban77/sonda:1.22.2\"\n", "");
        assert_eq!(workshop(&empty).image_pins, BTreeMap::new());
        let missing = empty.replace("[image_pins]\n", "");
        assert_eq!(workshop(&missing).image_pins, BTreeMap::new());
    }

    #[test]
    fn read_settings_file_names_the_path_and_the_key() {
        let dir = std::env::temp_dir().join(format!("nobs-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nobs.toml");
        fs::write(&path, without("title = \"Workshop one\"")).unwrap();
        let bad = read_settings_file(&path);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(bad, Err(format!("{}: title: missing", path.display())));
        let error = read_settings_file(&path).unwrap_err();
        assert!(
            error.starts_with(&format!("{}: ", path.display())),
            "{error}"
        );
    }
}
