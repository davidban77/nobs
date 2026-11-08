use std::env;
use std::fs;
use std::path::Path;

pub fn load_env(dir: &Path) -> Result<(), String> {
    for (key, value) in read_env(dir)? {
        if env::var_os(&key).is_none() {
            env::set_var(key, value);
        }
    }
    Ok(())
}

pub fn read_env(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for name in [".env", ".setup.env"] {
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
                if !pairs.iter().any(|(seen, _)| seen == key) {
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
