use std::ffi::OsString;
use std::fs;
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::str::Chars;

#[path = "support/tree.rs"]
mod tree;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use nobs_cli::{Cli, Command, WorkshopCli};
use tree::commands;

const STAND_IN: &str = "1";
const WORKSHOP_NAMES: [&str; 2] = [STAND_IN, "nobs-t-one"];
const SHOWN_AS_RUN: [ErrorKind; 2] = [ErrorKind::DisplayHelp, ErrorKind::DisplayVersion];
const LAUNCHERS: [&str; 2] = ["orb -m clab <path>", "./nobs"];

struct Line {
    number: usize,
    text: String,
    typed: bool,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn as_nobs(text: &str) -> Option<String> {
    ["nobs"].iter().chain(&LAUNCHERS).find_map(|start| {
        let rest = text.strip_prefix(start)?;
        (rest.is_empty() || rest.starts_with(' ')).then(|| format!("nobs{rest}"))
    })
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

fn prompt(number: usize, line: &str) -> Option<Line> {
    let text = as_nobs(line.strip_prefix("$ ")?)?;
    Some(Line {
        number,
        text,
        typed: true,
    })
}

fn span(number: usize, text: &str) -> Option<Line> {
    let text = as_nobs(text.replace('\n', " ").trim())?;
    Some(Line {
        number,
        text,
        typed: false,
    })
}

fn markdown_lines(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut prose = String::new();
    let mut fenced = false;
    for (index, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if fenced {
            lines.extend(prompt(index + 1, line));
        } else {
            prose.push_str(line);
        }
        prose.push('\n');
    }
    for (offset, code) in code_spans(&prose) {
        lines.extend(span(line_of(&prose, offset), &code.replace("\\|", "|")));
    }
    lines
}

fn code_spans(prose: &str) -> Vec<(usize, &str)> {
    let mut runs = Vec::new();
    let mut at = 0;
    while let Some(found) = prose[at..].find('`') {
        let start = at + found;
        let length = prose[start..].len() - prose[start..].trim_start_matches('`').len();
        runs.push((start, length));
        at = start + length;
    }
    let mut spans = Vec::new();
    let mut index = 0;
    while index < runs.len() {
        let (start, length) = runs[index];
        match (index + 1..runs.len()).find(|&next| runs[next].1 == length) {
            Some(close) => {
                spans.push((start, &prose[start + length..runs[close].0]));
                index = close + 1;
            }
            None => index += 1,
        }
    }
    spans
}

fn html_lines(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    for (open, close) in [("<code>", "</code>"), ("<div class=\"bar\">", "</div>")] {
        for (offset, inner) in elements(text, open, close) {
            lines.extend(span(line_of(text, offset), &decoded(inner)));
        }
    }
    for (offset, inner) in elements(text, "<pre>", "</pre>") {
        let first = line_of(text, offset);
        for (index, line) in inner.split('\n').enumerate() {
            lines.extend(prompt(first + index, &decoded(line)));
        }
    }
    lines
}

fn elements<'a>(text: &'a str, open: &str, close: &str) -> Vec<(usize, &'a str)> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = text[at..].find(open) {
        let body = at + start + open.len();
        let end = body + text[body..].find(close).expect("an element is closed");
        found.push((body, &text[body..end]));
        at = end + close.len();
    }
    found
}

fn decoded(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(character),
            _ => {}
        }
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

fn words(text: &str) -> Vec<OsString> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' => quoted = !quoted,
            '<' if !quoted => match placeholder(&mut chars) {
                true => word.push_str(STAND_IN),
                false => break,
            },
            '|' | '>' if !quoted => break,
            ' ' if !quoted && (!word.starts_with('[') || word.ends_with(']')) => {
                words.extend(notation(&word));
                word.clear();
            }
            _ => word.push(character),
        }
    }
    words.extend(notation(&word));
    words
}

fn notation(word: &str) -> Vec<OsString> {
    if word.is_empty() || word == "[OPTIONS]" || word == "..." {
        return Vec::new();
    }
    match word
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        Some(inner) if inner.starts_with('-') => inner.split(' ').map(stand_in).collect(),
        Some(_) => vec![OsString::from(STAND_IN)],
        None => vec![stand_in(word)],
    }
}

fn stand_in(word: &str) -> OsString {
    let name = word.strip_suffix("...").unwrap_or(word);
    let synopsis = name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit());
    OsString::from(if synopsis { STAND_IN } else { word })
}

fn placeholder(chars: &mut Peekable<Chars>) -> bool {
    let rest: String = chars.clone().collect();
    let Some(name) = rest.split_once('>').map(|(name, _)| name) else {
        return false;
    };
    let named = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_'));
    if named {
        chars.nth(name.chars().count());
    }
    named
}

fn parsed<T>(result: Result<T, clap::Error>) -> Result<Option<T>, String> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if SHOWN_AS_RUN.contains(&error.kind()) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn command_paths() -> Vec<String> {
    let cli = Cli::command();
    let workshop = WorkshopCli::command();
    let mut found = commands(&cli, "nobs");
    found.extend(commands(&workshop, "nobs"));
    for name in WORKSHOP_NAMES {
        found.extend(commands(&workshop, &format!("nobs {name}")));
    }
    found.into_iter().map(|(path, _)| path).collect()
}

fn accepted(words: &[OsString], typed: bool) -> Result<(), String> {
    let path: Vec<_> = words.iter().map(|word| word.to_string_lossy()).collect();
    match parses(words) {
        Err(_) if !typed && command_paths().contains(&path.join(" ")) => Ok(()),
        result => result,
    }
}

fn parses(words: &[OsString]) -> Result<(), String> {
    let Some(cli) = parsed(Cli::try_parse_from(words))? else {
        return Ok(());
    };
    let Some(Command::External(rest)) = cli.command else {
        return Ok(());
    };
    let first = rest[0].to_string_lossy();
    let args = if WORKSHOP_NAMES.contains(&first.as_ref()) {
        &rest[1..]
    } else if WorkshopCli::command().find_subcommand(&*first).is_some() {
        &rest[..]
    } else {
        return Err(format!(
            "{first} is no command of nobs and no workshop name the guides use"
        ));
    };
    parsed(WorkshopCli::try_parse_from(args)).map(|_| ())
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|found| found == extension)
}

fn guide_files() -> Vec<PathBuf> {
    let mut pages: Vec<PathBuf> = fs::read_dir(root().join("docs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| has_extension(path, "html"))
        .collect();
    pages.sort();
    pages.insert(0, root().join("README.md"));
    pages
}

fn command_lines(path: &Path) -> Vec<Line> {
    let text = fs::read_to_string(path).unwrap();
    let mut lines = match has_extension(path, "md") {
        true => markdown_lines(&text),
        false => html_lines(&text),
    };
    lines.sort_by_key(|line| line.number);
    lines
}

fn first_paragraph(error: &str) -> String {
    let paragraph = error.trim().split("\n\n").next().unwrap_or_default();
    paragraph.replace('\n', "\n    ")
}

#[test]
fn every_command_line_of_the_guides_is_accepted_by_nobs() {
    let mut refused = Vec::new();
    let mut counts = Vec::new();
    for path in guide_files() {
        let file = path.strip_prefix(root()).unwrap().display().to_string();
        let lines = command_lines(&path);
        counts.push(format!("{file} {}", lines.len()));
        for line in lines {
            let words = words(&line.text);
            if let Err(error) = accepted(&words, line.typed) {
                refused.push(format!(
                    "{file}:{}: {}\n  words: {words:?}\n  {}",
                    line.number,
                    line.text,
                    first_paragraph(&error)
                ));
            }
        }
    }
    println!("command lines checked: {}", counts.join(", "));
    assert!(
        refused.is_empty(),
        "{} command lines of the guides are refused:\n{}",
        refused.len(),
        refused.join("\n")
    );
}
