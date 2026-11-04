use std::env;
use std::fs;
use std::path::Path;

pub fn load_env(dir: &Path) -> Result<(), String> {
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
                if env::var_os(key).is_none() {
                    env::set_var(key, value);
                }
            }
        }
    }
    Ok(())
}

fn parse_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), unquote(value)))
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
    fn the_key_is_trimmed_and_the_value_is_not() {
        assert_eq!(parse_line("A = 1"), Some(("A", " 1")));
    }
}
