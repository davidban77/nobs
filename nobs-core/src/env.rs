use std::env;
use std::fs;
use std::path::Path;

use crate::status::{self, service_named, Service, SERVICES};

pub fn load_env(dir: &Path) -> Result<(), String> {
    let process = |name: &str| env::var(name).ok();
    let held: Vec<Service> = SERVICES
        .iter()
        .map(|row| row.service)
        .filter(|service| status::given(*service, &[], &[&process]).is_some())
        .collect();
    for (key, value) in read_env(dir)? {
        let shell_holds = service_named(&key).is_some_and(|service| held.contains(&service));
        if env::var_os(&key).is_none() && !shell_holds {
            env::set_var(key, value);
        }
    }
    Ok(())
}

pub fn read_env(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for name in [".env", ".setup.env"] {
        let earlier: Vec<Service> = pairs
            .iter()
            .filter_map(|(key, _)| service_named(key))
            .collect();
        let path = dir.join(name);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if let Some((key, value)) = parse_line(line) {
                if key.is_empty() {
                    return Err(format!("{}: line {}: empty key", path.display(), index + 1));
                }
                if key.contains('\0') || value.contains('\0') {
                    return Err(format!("{}: line {}: NUL byte", path.display(), index + 1));
                }
                let named_earlier =
                    service_named(key).is_some_and(|service| earlier.contains(&service));
                if !named_earlier && !pairs.iter().any(|(seen, _)| seen == key) {
                    pairs.push((key.to_string(), value.to_string()));
                }
            }
        }
    }
    Ok(pairs)
}

fn parse_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), unquote(value.trim())))
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nobs-env-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            fs::write(dir.join(file), text).unwrap();
        }
        dir
    }

    #[test]
    fn read_env_gives_dot_env_then_setup_env_and_keeps_the_first_value_of_a_key() {
        let dir = folder(
            "order",
            &[(".env", "A=1\nB=2\n"), (".setup.env", "A=9\nC=3\n")],
        );
        let pairs = read_env(&dir);
        let _ = fs::remove_dir_all(&dir);
        let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
        assert_eq!(
            pairs,
            Ok(vec![pair("A", "1"), pair("B", "2"), pair("C", "3")])
        );
    }

    fn metrics_from(dir: &std::path::Path) -> String {
        let pairs = read_env(dir).unwrap();
        let file = |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        status::address(Service::Metrics, &[], &[&file])
    }

    #[test]
    fn read_env_takes_a_setting_from_the_first_file_that_holds_it() {
        let dir = folder(
            "first-file",
            &[
                (
                    ".env",
                    "METRICS_URL=http://127.0.0.1:19001\nLOKI_URL=http://127.0.0.1:19003\n",
                ),
                (
                    ".setup.env",
                    "PROMETHEUS_URL=http://127.0.0.1:19002\nLOGS_URL=http://127.0.0.1:19004\nA=1\n",
                ),
            ],
        );
        let pairs = read_env(&dir);
        let _ = fs::remove_dir_all(&dir);
        let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
        assert_eq!(
            pairs,
            Ok(vec![
                pair("METRICS_URL", "http://127.0.0.1:19001"),
                pair("LOKI_URL", "http://127.0.0.1:19003"),
                pair("A", "1"),
            ])
        );
    }

    #[test]
    fn a_blank_name_in_dot_env_blocks_a_set_name_in_setup_env() {
        for (name, other) in [
            ("PROMETHEUS_URL", "PROMETHEUS_URL"),
            ("METRICS_URL", "PROMETHEUS_URL"),
        ] {
            let dir = folder(
                "blank-blocks",
                &[
                    (".env", format!("{name}=\n").as_str()),
                    (
                        ".setup.env",
                        format!("{other}=http://127.0.0.1:19002\n").as_str(),
                    ),
                ],
            );
            let metrics = metrics_from(&dir);
            let _ = fs::remove_dir_all(&dir);
            assert_eq!(
                metrics,
                status::row(Service::Metrics).default,
                "{name} {other}"
            );
        }
    }

    #[test]
    fn a_blank_new_name_and_a_set_old_name_in_one_file_give_the_old() {
        let dir = folder(
            "blank-new",
            &[(
                ".env",
                "METRICS_URL= \nPROMETHEUS_URL=http://127.0.0.1:19002\n",
            )],
        );
        let metrics = metrics_from(&dir);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(metrics, "http://127.0.0.1:19002");
    }

    #[test]
    fn load_env_keeps_the_setting_where_the_shell_holds_it() {
        env::set_var("PROMETHEUS_URL", "http://127.0.0.1:19001");
        env::set_var("LOGS_URL", "http://127.0.0.1:19003");
        for name in ["METRICS_URL", "LOKI_URL", "ALERTS_URL", "ALERTMANAGER_URL"] {
            env::remove_var(name);
        }
        let dir = folder(
            "shell-holds",
            &[(
                ".env",
                "METRICS_URL=http://127.0.0.1:19002\nLOKI_URL=http://127.0.0.1:19004\nALERTS_URL=http://127.0.0.1:19005\nALERTMANAGER_URL=http://127.0.0.1:19006\n",
            )],
        );
        let loaded = load_env(&dir);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(loaded, Ok(()));
        assert_eq!(env::var("METRICS_URL").ok(), None);
        assert_eq!(env::var("LOKI_URL").ok(), None);
        assert_eq!(
            env::var("ALERTS_URL").as_deref(),
            Ok("http://127.0.0.1:19005")
        );
        assert_eq!(
            env::var("ALERTMANAGER_URL").as_deref(),
            Ok("http://127.0.0.1:19006")
        );
    }

    #[test]
    fn a_blank_name_in_the_shell_does_not_block_the_labs_file() {
        env::set_var("METRICS_URL", " ");
        env::remove_var("PROMETHEUS_URL");
        let dir = folder(
            "shell-blank",
            &[(".env", "PROMETHEUS_URL=http://127.0.0.1:19002\n")],
        );
        let loaded = load_env(&dir);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(loaded, Ok(()));
        let process = |name: &str| env::var(name).ok();
        assert_eq!(
            status::address(Service::Metrics, &[], &[&process]),
            "http://127.0.0.1:19002"
        );
    }

    #[test]
    fn read_env_names_an_empty_key_with_its_file_and_line() {
        let dir = folder("empty-key", &[(".env", "A=1\n=x\n")]);
        let error = read_env(&dir).unwrap_err();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(
            error,
            format!("{}: line 2: empty key", dir.join(".env").display())
        );
    }

    #[test]
    fn a_comment_and_a_blank_line_give_nothing() {
        assert_eq!(parse_line("# A=1"), None);
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("   "), None);
    }

    #[test]
    fn an_export_prefix_is_ignored() {
        assert_eq!(parse_line("export A=1"), Some(("A", "1")));
    }

    #[test]
    fn one_pair_of_surrounding_quotes_is_stripped() {
        assert_eq!(parse_line("A=\"x y\""), Some(("A", "x y")));
        assert_eq!(parse_line("A='x y'"), Some(("A", "x y")));
    }

    #[test]
    fn a_dollar_reference_stays_literal() {
        assert_eq!(parse_line("A=$B"), Some(("A", "$B")));
    }

    #[test]
    fn spaces_around_the_equals_sign_are_trimmed() {
        assert_eq!(parse_line("A = 1"), Some(("A", "1")));
    }

    #[test]
    fn a_quoted_value_after_spaces_loses_its_quotes() {
        assert_eq!(parse_line("A = \"x\""), Some(("A", "x")));
    }
}
