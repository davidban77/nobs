#[path = "support/product_code.rs"]
mod product_code;

use nobs_core::status::SERVICES;
use product_code::product_code;

const REAL_THINGS: [&str; 9] = [
    "into Infrahub at",
    "Open Infrahub at",
    "Open Grafana → Explore → Loki and run:",
    "Alertmanager's repeat_interval",
    "workshop-related Alertmanager silence(s)",
    "in Alertmanager at",
    "Loki match count=",
    "no Loki match after",
    "re-loaded Infrahub schema",
];

const HELP_MARKS: [&str; 3] = ["help = ", "about = ", ".help("];

fn without_table(code: &str) -> String {
    match code.find("pub const SERVICES") {
        Some(start) => {
            let end = start + code[start..].find("\n];").unwrap();
            format!("{}{}", &code[..start], &code[end..])
        }
        None => code.to_string(),
    }
}

fn literals(code: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = code.chars().collect();
    let at_char = |index: usize| chars.get(index).copied();
    let mut found = Vec::new();
    let mut line = 0;
    let mut at = 0;
    while at < chars.len() {
        match chars[at] {
            '\n' => line += 1,
            '/' if at_char(at + 1) == Some('/') => {
                while at_char(at + 1).is_some_and(|c| c != '\n') {
                    at += 1;
                }
            }
            '\'' if at_char(at + 1) == Some('\\') && at_char(at + 3) == Some('\'') => at += 3,
            '\'' if at_char(at + 2) == Some('\'') => at += 2,
            'r' if at == 0 || !(chars[at - 1].is_alphanumeric() || chars[at - 1] == '_') => {
                let hashes = chars[at + 1..].iter().take_while(|c| **c == '#').count();
                if at_char(at + 1 + hashes) == Some('"') {
                    let start = at + 2 + hashes;
                    let close: String = std::iter::once('"')
                        .chain(std::iter::repeat_n('#', hashes))
                        .collect();
                    let rest: String = chars[start..].iter().collect();
                    let text = &rest[..rest.find(&close).unwrap()];
                    found.push((line, text.to_string()));
                    line += text.matches('\n').count();
                    at = start + text.chars().count() + hashes;
                }
            }
            '"' => {
                let start_line = line;
                let mut text = String::new();
                at += 1;
                while chars[at] != '"' {
                    if chars[at] == '\\' {
                        text.push(chars[at]);
                        at += 1;
                    }
                    if chars[at] == '\n' {
                        line += 1;
                    }
                    text.push(chars[at]);
                    at += 1;
                }
                found.push((start_line, text));
            }
            _ => {}
        }
        at += 1;
    }
    found
}

#[test]
fn a_tool_name_is_spelled_only_in_the_table_a_help_text_or_a_text_of_a_real_thing() {
    let tools: Vec<&str> = SERVICES
        .iter()
        .filter(|row| row.label() != row.word)
        .map(|row| row.tool)
        .collect();
    assert_eq!(tools.len(), 5, "{tools:?}");

    let files = product_code();
    assert!(files.len() > 10, "{}", files.len());
    let mut strays = Vec::new();
    let mut unused = REAL_THINGS.to_vec();
    for (path, code) in files {
        let code = without_table(&code);
        let lines: Vec<&str> = code.lines().collect();
        for (line, text) in literals(&code) {
            if HELP_MARKS.iter().any(|mark| lines[line].contains(mark)) {
                continue;
            }
            unused.retain(|thing| !text.contains(thing));
            if tools.iter().any(|tool| text.contains(tool))
                && !REAL_THINGS.iter().any(|thing| text.contains(thing))
            {
                strays.push(format!("{}:{}: {text}", path.display(), line + 1));
            }
        }
    }

    assert!(strays.is_empty(), "{strays:#?}");
    assert!(unused.is_empty(), "{unused:?}");
}
