use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::device::{self, Error, Transport};
use crate::intended::{self, Failure};
use crate::lab::TopologyNode;

const CONFIG: &str = "config";
const ERROR: &str = "error";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub name: String,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Answered { lines: Vec<String>, missing: usize },
    Failed(String),
    NotTried,
    NotChecked(String),
    NoIntendedText(String),
}

pub fn walk(
    nodes: &[TopologyNode],
    transport: &dyn Transport,
    intended_of: &dyn Fn(&TopologyNode) -> Result<String, Failure>,
    each: &mut dyn FnMut(&Reading),
) -> Vec<Reading> {
    let mut login_failed = false;
    nodes
        .iter()
        .map(|node| {
            let verdict = if login_failed {
                Verdict::NotTried
            } else {
                verdict_of(node, transport, intended_of).unwrap_or_else(|error| {
                    login_failed = true;
                    Verdict::Failed(error.to_string())
                })
            };
            let reading = Reading {
                name: node.name.clone(),
                verdict,
            };
            each(&reading);
            reading
        })
        .collect()
}

fn verdict_of(
    node: &TopologyNode,
    transport: &dyn Transport,
    intended_of: &dyn Fn(&TopologyNode) -> Result<String, Failure>,
) -> Result<Verdict, Error> {
    let intended = match intended_of(node) {
        Ok(text) => text,
        Err(Failure::NoTemplate(words)) => return Ok(Verdict::NotChecked(words)),
        Err(Failure::Other(words)) => return Ok(Verdict::NoIntendedText(words)),
    };
    let answer = device::step(node, |host, login, driver| {
        transport.run(host, login, driver.running_config)
    });
    match answer {
        Ok(output) if output.code != 0 => Ok(Verdict::Failed(exit_words(output.code))),
        Ok(output) => {
            let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
            let missing = intended::missing(&intended::compare(&intended, &lines));
            Ok(Verdict::Answered { lines, missing })
        }
        Err(error @ (Error::AuthenticationFailed | Error::MissingCredentials)) => Err(error),
        Err(error) => Ok(Verdict::Failed(error.to_string())),
    }
}

pub fn exit_words(code: i32) -> String {
    format!("the device ended with exit code {code}")
}

pub fn width(nodes: &[TopologyNode]) -> usize {
    nodes
        .iter()
        .map(|node| node.name.chars().count())
        .max()
        .unwrap_or(0)
}

pub fn line(width: usize, reading: &Reading) -> String {
    format!("{:<width$}  {}", reading.name, words(&reading.verdict))
}

fn words(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Answered { missing: 0, .. } => "in sync".to_string(),
        Verdict::Answered { missing, .. } => format!("{missing} missing"),
        Verdict::Failed(words) => format!("failed: {words}"),
        Verdict::NotTried => "not tried".to_string(),
        Verdict::NotChecked(words) => format!("not checked: {words}"),
        Verdict::NoIntendedText(words) => format!("no intended text: {words}"),
    }
}

pub fn exit_code(readings: &[Reading]) -> i32 {
    let broken = readings.iter().any(|reading| match &reading.verdict {
        Verdict::Answered { missing, .. } => *missing > 0,
        Verdict::Failed(_) | Verdict::NoIntendedText(_) | Verdict::NotTried => true,
        Verdict::NotChecked(_) => false,
    });
    i32::from(broken)
}

pub fn make_folder(folder: &Path) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(folder)
}

pub fn file(folder: &Path, reading: &Reading) -> Option<PathBuf> {
    kept(folder, reading).map(|(path, _)| path)
}

fn kept(folder: &Path, reading: &Reading) -> Option<(PathBuf, String)> {
    let (extension, text) = match &reading.verdict {
        Verdict::Answered { lines, .. } => (
            CONFIG,
            lines.iter().map(|line| format!("{line}\n")).collect(),
        ),
        Verdict::Failed(words) => (ERROR, words.clone()),
        Verdict::NotTried | Verdict::NotChecked(_) | Verdict::NoIntendedText(_) => return None,
    };
    Some((folder.join(format!("{}.{extension}", reading.name)), text))
}

pub fn save(folder: &Path, reading: &Reading) -> io::Result<()> {
    let Some((path, text)) = kept(folder, reading) else {
        return Ok(());
    };
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(text.as_bytes())
}

pub fn load(folder: &Path) -> Vec<(String, Result<Vec<String>, String>)> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<_> = entries
        .flatten()
        .filter_map(|entry| loaded(&entry.path()))
        .collect();
    found.sort_by(|one, other| one.0.cmp(&other.0));
    found
}

fn loaded(path: &Path) -> Option<(String, Result<Vec<String>, String>)> {
    let name = path.file_stem()?.to_str()?.to_string();
    let text = || fs::read_to_string(path).ok();
    let fetched = match path.extension()?.to_str()? {
        CONFIG => Ok(text()?.lines().map(str::to_string).collect()),
        ERROR => Err(text()?),
        _ => return None,
    };
    Some((name, fetched))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;
    use crate::device::FakeTransport;
    use crate::runner::Output;

    const INTENDED: &str = "set / a b on\nset / c d on\n";
    const SYNCED: &str = "set / a b on\nset / c d on\nset / e f on\n";
    const LACKING: &str = "set / a b off\n";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("nobs-check-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn folder(&self) -> PathBuf {
            self.0.join("check")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn node(name: &str, kind: &str) -> TopologyNode {
        TopologyNode {
            name: name.to_string(),
            kind: kind.to_string(),
            container: host(name),
        }
    }

    fn host(name: &str) -> String {
        format!("clab-{name}")
    }

    fn replied(code: i32, stdout: &str) -> Result<Output, Error> {
        Ok(Output {
            code,
            stdout: stdout.to_string(),
            stderr: String::new(),
        })
    }

    fn answers(list: Vec<(&str, Result<Output, Error>)>) -> FakeTransport {
        FakeTransport::per_host(
            list.into_iter()
                .map(|(name, answer)| (host(name), answer))
                .collect(),
        )
    }

    fn answered(stdout: &str, missing: usize) -> Verdict {
        Verdict::Answered {
            lines: stdout.lines().map(str::to_string).collect(),
            missing,
        }
    }

    fn reading(name: &str, verdict: Verdict) -> Reading {
        Reading {
            name: name.to_string(),
            verdict,
        }
    }

    fn logged_in() {
        std::env::set_var(device::USER, "admin");
        std::env::set_var(device::PASSWORD, "nobs-t-not-a-secret");
    }

    fn templated(_: &TopologyNode) -> Result<String, Failure> {
        Ok(INTENDED.to_string())
    }

    fn checked(
        nodes: &[TopologyNode],
        transport: &FakeTransport,
        intended_of: &dyn Fn(&TopologyNode) -> Result<String, Failure>,
    ) -> (Vec<Reading>, Vec<usize>) {
        let mut seen = Vec::new();
        let mut calls_seen = Vec::new();
        let readings = walk(nodes, transport, intended_of, &mut |reading| {
            seen.push(reading.clone());
            calls_seen.push(transport.calls().len());
        });
        assert_eq!(seen, readings);
        (readings, calls_seen)
    }

    fn fetch(name: &str) -> (String, String) {
        (host(name), "info flat".to_string())
    }

    #[test]
    fn a_device_that_does_not_answer_stops_no_other_and_the_first_and_the_last_of_a_second_kind_get_their_verdicts(
    ) {
        logged_in();
        let nodes = [
            node("nobs-t-srl1", "nokia_srlinux"),
            node("nobs-t-srl2", "nokia_srlinux"),
            node("nobs-t-srl3", "srl"),
            node("nobs-t-srl4", "srl"),
        ];
        let transport = answers(vec![
            ("nobs-t-srl1", replied(0, SYNCED)),
            (
                "nobs-t-srl2",
                Err(Error::Unreachable {
                    host: host("nobs-t-srl2"),
                    reason: "connection refused".to_string(),
                }),
            ),
            ("nobs-t-srl3", replied(0, LACKING)),
            ("nobs-t-srl4", replied(0, SYNCED)),
        ]);

        let (readings, calls_seen) = checked(&nodes, &transport, &templated);

        assert_eq!(
            readings,
            [
                reading("nobs-t-srl1", answered(SYNCED, 0)),
                reading(
                    "nobs-t-srl2",
                    Verdict::Failed("cannot reach clab-nobs-t-srl2: connection refused".into())
                ),
                reading("nobs-t-srl3", answered(LACKING, 2)),
                reading("nobs-t-srl4", answered(SYNCED, 0)),
            ]
        );
        assert_eq!(
            transport.calls(),
            [
                fetch("nobs-t-srl1"),
                fetch("nobs-t-srl2"),
                fetch("nobs-t-srl3"),
                fetch("nobs-t-srl4")
            ]
        );
        assert_eq!(calls_seen, [1, 2, 3, 4]);
    }

    #[test]
    fn a_refused_login_on_the_second_of_four_stops_the_walk_and_every_later_node_is_not_tried() {
        logged_in();
        let nodes = [
            node("nobs-t-srl1", "nokia_srlinux"),
            node("nobs-t-srl2", "srl"),
            node("nobs-t-srl3", "nokia_srlinux"),
            node("nobs-t-srl4", "srl"),
            node("nobs-t-ceos1", "ceos"),
        ];
        let transport = answers(vec![
            ("nobs-t-srl1", replied(0, SYNCED)),
            ("nobs-t-srl2", Err(Error::AuthenticationFailed)),
            ("nobs-t-srl3", replied(0, SYNCED)),
            ("nobs-t-srl4", replied(0, SYNCED)),
        ]);
        let intended_of = |node: &TopologyNode| match node.kind.as_str() {
            "ceos" => Err(Failure::NoTemplate("no template for kind ceos".into())),
            _ => templated(node),
        };

        let (readings, _) = checked(&nodes, &transport, &intended_of);

        assert_eq!(
            readings,
            [
                reading("nobs-t-srl1", answered(SYNCED, 0)),
                reading(
                    "nobs-t-srl2",
                    Verdict::Failed("authentication failed".into())
                ),
                reading("nobs-t-srl3", Verdict::NotTried),
                reading("nobs-t-srl4", Verdict::NotTried),
                reading("nobs-t-ceos1", Verdict::NotTried),
            ]
        );
        assert_eq!(
            transport.calls(),
            [fetch("nobs-t-srl1"), fetch("nobs-t-srl2")]
        );
        assert_eq!(exit_code(&readings), 1);
    }

    #[test]
    fn missing_credentials_stop_the_walk_as_a_refused_login_does_after_a_node_with_no_driver() {
        std::env::remove_var(device::USER);
        std::env::remove_var(device::PASSWORD);
        let nodes = [
            node("nobs-t-ceos1", "ceos"),
            node("nobs-t-srl1", "nokia_srlinux"),
            node("nobs-t-srl2", "srl"),
            node("nobs-t-ceos2", "ceos"),
        ];
        let transport = answers(vec![
            ("nobs-t-srl1", replied(0, SYNCED)),
            ("nobs-t-srl2", replied(0, SYNCED)),
        ]);

        let (readings, _) = checked(&nodes, &transport, &templated);

        assert_eq!(
            readings,
            [
                reading(
                    "nobs-t-ceos1",
                    Verdict::Failed("no driver for the kind ceos yet".into())
                ),
                reading(
                    "nobs-t-srl1",
                    Verdict::Failed(Error::MissingCredentials.to_string())
                ),
                reading("nobs-t-srl2", Verdict::NotTried),
                reading("nobs-t-ceos2", Verdict::NotTried),
            ]
        );
        assert!(transport.calls().is_empty());
    }

    #[test]
    fn a_kind_with_no_driver_fails_with_no_login_and_the_walk_goes_on() {
        logged_in();
        let nodes = [
            node("nobs-t-ceos1", "ceos"),
            node("nobs-t-srl1", "nokia_srlinux"),
            node("nobs-t-srl2", "srl"),
            node("nobs-t-srl3", "srl"),
        ];
        let transport = answers(vec![
            ("nobs-t-srl1", replied(0, SYNCED)),
            ("nobs-t-srl2", replied(0, SYNCED)),
            ("nobs-t-srl3", replied(0, SYNCED)),
        ]);

        let (readings, _) = checked(&nodes, &transport, &templated);

        assert_eq!(
            readings[0],
            reading(
                "nobs-t-ceos1",
                Verdict::Failed("no driver for the kind ceos yet".into())
            )
        );
        assert_eq!(readings[3], reading("nobs-t-srl3", answered(SYNCED, 0)));
        assert_eq!(
            transport.calls(),
            [
                fetch("nobs-t-srl1"),
                fetch("nobs-t-srl2"),
                fetch("nobs-t-srl3")
            ]
        );
        assert_eq!(exit_code(&readings), 1);
    }

    #[test]
    fn a_node_with_no_template_is_not_fetched_and_leaves_the_exit_code_at_zero() {
        logged_in();
        let nodes = [
            node("nobs-t-srl1", "nokia_srlinux"),
            node("nobs-t-srl2", "srl"),
            node("nobs-t-srl3", "nokia_srlinux"),
            node("nobs-t-srl4", "srl"),
            node("nobs-t-linux1", "linux"),
        ];
        let transport = answers(vec![
            ("nobs-t-srl1", replied(0, SYNCED)),
            ("nobs-t-srl2", replied(0, LACKING)),
            ("nobs-t-srl3", replied(0, SYNCED)),
            ("nobs-t-srl4", replied(0, SYNCED)),
        ]);
        let no_template = |kind: &str| Failure::NoTemplate(format!("no template for kind {kind}"));
        let intended_of = |node: &TopologyNode| match node.name.as_str() {
            "nobs-t-srl2" | "nobs-t-linux1" => Err(no_template(&node.kind)),
            _ => templated(node),
        };

        let (readings, _) = checked(&nodes, &transport, &intended_of);

        assert_eq!(
            readings[1],
            reading(
                "nobs-t-srl2",
                Verdict::NotChecked("no template for kind srl".into())
            )
        );
        assert_eq!(
            readings[4],
            reading(
                "nobs-t-linux1",
                Verdict::NotChecked("no template for kind linux".into())
            )
        );
        assert_eq!(
            transport.calls(),
            [
                fetch("nobs-t-srl1"),
                fetch("nobs-t-srl3"),
                fetch("nobs-t-srl4")
            ]
        );
        assert_eq!(exit_code(&readings), 0);
    }

    #[test]
    fn a_node_whose_intended_text_fails_is_not_fetched_and_gives_exit_one() {
        logged_in();
        let nodes = [
            node("nobs-t-srl1", "nokia_srlinux"),
            node("nobs-t-srl2", "srl"),
            node("nobs-t-srl3", "nokia_srlinux"),
            node("nobs-t-srl4", "srl"),
        ];
        let transport = answers(vec![
            ("nobs-t-srl1", replied(0, SYNCED)),
            ("nobs-t-srl2", replied(0, SYNCED)),
            ("nobs-t-srl3", replied(0, SYNCED)),
            ("nobs-t-srl4", replied(0, SYNCED)),
        ]);
        let intended_of = |node: &TopologyNode| match node.name.as_str() {
            "nobs-t-srl2" => Err(Failure::Other("nobs-t-source-error".into())),
            _ => templated(node),
        };

        let (readings, _) = checked(&nodes, &transport, &intended_of);

        assert_eq!(
            readings[1],
            reading(
                "nobs-t-srl2",
                Verdict::NoIntendedText("nobs-t-source-error".into())
            )
        );
        assert_eq!(
            transport.calls(),
            [
                fetch("nobs-t-srl1"),
                fetch("nobs-t-srl3"),
                fetch("nobs-t-srl4")
            ]
        );
        assert_eq!(exit_code(&readings), 1);
    }

    #[test]
    fn a_device_ending_with_a_code_other_than_zero_fails_with_the_code_and_none_of_its_text() {
        logged_in();
        let scratch = Scratch::new("exit-code");
        let nodes = [
            node("nobs-t-srl1", "nokia_srlinux"),
            node("nobs-t-srl2", "srl"),
        ];
        let transport = answers(vec![
            ("nobs-t-srl1", replied(1, "set / nobs-t-sentinel on\n")),
            ("nobs-t-srl2", replied(0, SYNCED)),
        ]);

        let (readings, _) = checked(&nodes, &transport, &templated);
        make_folder(&scratch.folder()).unwrap();
        for reading in &readings {
            save(&scratch.folder(), reading).unwrap();
        }

        assert_eq!(
            readings[0],
            reading(
                "nobs-t-srl1",
                Verdict::Failed("the device ended with exit code 1".into())
            )
        );
        let printed = line(width(&nodes), &readings[0]);
        assert_eq!(
            printed,
            "nobs-t-srl1  failed: the device ended with exit code 1"
        );
        assert_eq!(
            load(&scratch.folder())[0],
            (
                "nobs-t-srl1".to_string(),
                Err("the device ended with exit code 1".to_string())
            )
        );
        assert_eq!(exit_code(&readings), 1);
    }

    #[test]
    fn the_exit_code_is_zero_only_when_every_checked_device_is_in_sync() {
        let synced = || reading("nobs-t-srl1", answered(SYNCED, 0));
        let skipped = |name: &str| reading(name, Verdict::NotChecked("no template".into()));
        let with = |other: Reading| vec![synced(), other, skipped("nobs-t-linux1")];
        let cases = [
            (vec![synced(), synced(), skipped("nobs-t-linux1")], 0),
            (Vec::new(), 0),
            (vec![skipped("nobs-t-linux1"), skipped("nobs-t-linux2")], 0),
            (with(reading("nobs-t-srl2", answered(LACKING, 2))), 1),
            (
                with(reading("nobs-t-srl2", Verdict::Failed("no answer".into()))),
                1,
            ),
            (
                with(reading(
                    "nobs-t-srl2",
                    Verdict::NoIntendedText("nobs-t-source-error".into()),
                )),
                1,
            ),
            (with(reading("nobs-t-srl2", Verdict::NotTried)), 1),
        ];
        for (case, (readings, code)) in cases.into_iter().enumerate() {
            assert_eq!(exit_code(&readings), code, "case {case}");
        }
    }

    #[test]
    fn each_line_is_the_name_padded_to_the_longest_name_two_spaces_and_the_words() {
        let nodes = [
            node("nobs-t-srl1", "nokia_srlinux"),
            node("nobs-t-leaf10", "srl"),
            node("a", "linux"),
        ];
        let width = width(&nodes);
        let cases = [
            (
                reading("nobs-t-srl1", answered(SYNCED, 0)),
                "nobs-t-srl1    in sync",
            ),
            (
                reading("nobs-t-leaf10", answered(LACKING, 2)),
                "nobs-t-leaf10  2 missing",
            ),
            (
                reading("a", Verdict::Failed("authentication failed".into())),
                "a              failed: authentication failed",
            ),
            (reading("a", Verdict::NotTried), "a              not tried"),
            (
                reading(
                    "a",
                    Verdict::NotChecked("no template for kind linux".into()),
                ),
                "a              not checked: no template for kind linux",
            ),
            (
                reading("a", Verdict::NoIntendedText("nobs-t-source-error".into())),
                "a              no intended text: nobs-t-source-error",
            ),
        ];
        assert_eq!(width, 13);
        assert_eq!(super::width(&[]), 0);
        for (reading, expected) in cases {
            assert_eq!(line(width, &reading), expected);
        }
    }

    #[test]
    fn making_the_folder_fails_on_an_existing_folder_or_file() {
        let scratch = Scratch::new("existing");
        make_folder(&scratch.folder()).unwrap();
        let file = scratch.0.join("file");
        fs::write(&file, "").unwrap();

        assert_eq!(
            make_folder(&scratch.folder()).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            make_folder(&file).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn the_folder_is_700_and_each_saved_file_600() {
        let scratch = Scratch::new("modes");
        let folder = scratch.folder();
        make_folder(&folder).unwrap();
        save(&folder, &reading("nobs-t-srl1", answered(SYNCED, 0))).unwrap();
        save(
            &folder,
            &reading("nobs-t-srl2", Verdict::Failed("x".into())),
        )
        .unwrap();

        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&folder), 0o700);
        assert_eq!(mode(&folder.join("nobs-t-srl1.config")), 0o600);
        assert_eq!(mode(&folder.join("nobs-t-srl2.error")), 0o600);
    }

    #[test]
    fn save_writes_a_config_for_an_answer_an_error_for_a_failure_and_nothing_for_the_rest() {
        let scratch = Scratch::new("files");
        let folder = scratch.folder();
        make_folder(&folder).unwrap();
        let readings = [
            reading("nobs-t-srl1", answered(SYNCED, 0)),
            reading("nobs-t-srl2", answered(LACKING, 2)),
            reading(
                "nobs-t-srl3",
                Verdict::Failed("authentication failed".into()),
            ),
            reading("nobs-t-srl4", Verdict::NotTried),
            reading("nobs-t-srl5", Verdict::NotChecked("no template".into())),
            reading(
                "nobs-t-srl6",
                Verdict::NoIntendedText("nobs-t-source-error".into()),
            ),
        ];
        for reading in &readings {
            save(&folder, reading).unwrap();
        }

        let mut names: Vec<String> = fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "nobs-t-srl1.config",
                "nobs-t-srl2.config",
                "nobs-t-srl3.error"
            ]
        );
        let read = |name: &str| fs::read_to_string(folder.join(name)).unwrap();
        assert_eq!(read("nobs-t-srl1.config"), SYNCED);
        assert_eq!(read("nobs-t-srl2.config"), LACKING);
        assert_eq!(read("nobs-t-srl3.error"), "authentication failed");
    }

    #[test]
    fn file_is_a_config_for_an_answer_an_error_for_a_failure_and_none_for_the_rest() {
        let folder = Path::new("nobs-t-folder");
        let cases = [
            (
                reading("nobs-t-srl1", answered(LACKING, 2)),
                Some(folder.join("nobs-t-srl1.config")),
            ),
            (
                reading("nobs-t-srl2", Verdict::Failed("no answer".into())),
                Some(folder.join("nobs-t-srl2.error")),
            ),
            (reading("nobs-t-srl3", Verdict::NotTried), None),
            (
                reading("nobs-t-srl4", Verdict::NotChecked("no template".into())),
                None,
            ),
            (
                reading(
                    "nobs-t-srl5",
                    Verdict::NoIntendedText("nobs-t-source-error".into()),
                ),
                None,
            ),
        ];
        for (reading, expected) in cases {
            assert_eq!(file(folder, &reading), expected, "{}", reading.name);
        }
    }

    #[test]
    fn load_gives_back_what_save_wrote_in_name_order() {
        let scratch = Scratch::new("round-trip");
        let folder = scratch.folder();
        make_folder(&folder).unwrap();
        let readings = [
            reading("nobs-t-srl3", answered("\nset / a b on\n  \n", 1)),
            reading("nobs-t-srl1", answered(SYNCED, 0)),
            reading(
                "nobs-t-srl2",
                Verdict::Failed("no answer in 10 seconds".into()),
            ),
            reading("nobs-t-srl4", answered("", 2)),
        ];
        for reading in &readings {
            save(&folder, reading).unwrap();
        }
        let lines = |text: &str| text.lines().map(str::to_string).collect::<Vec<_>>();

        assert_eq!(
            load(&folder),
            [
                ("nobs-t-srl1".to_string(), Ok(lines(SYNCED))),
                (
                    "nobs-t-srl2".to_string(),
                    Err("no answer in 10 seconds".to_string())
                ),
                (
                    "nobs-t-srl3".to_string(),
                    Ok(vec![String::new(), "set / a b on".into(), "  ".into()])
                ),
                ("nobs-t-srl4".to_string(), Ok(Vec::new())),
            ]
        );
    }

    #[test]
    fn load_skips_a_file_it_cannot_read_or_does_not_know() {
        let scratch = Scratch::new("bad-files");
        let folder = scratch.folder();
        make_folder(&folder).unwrap();
        fs::write(folder.join("nobs-t-srl1.config"), [0xff, 0xfe, 0xfd]).unwrap();
        fs::create_dir(folder.join("nobs-t-srl2.config")).unwrap();
        fs::write(folder.join("nobs-t-srl3.txt"), "set / a b on\n").unwrap();
        fs::write(folder.join("nobs-t-srl4"), "set / a b on\n").unwrap();
        fs::write(folder.join("nobs-t-srl5.error"), "authentication failed").unwrap();

        assert_eq!(
            load(&folder),
            [(
                "nobs-t-srl5".to_string(),
                Err("authentication failed".to_string())
            )]
        );
    }

    #[test]
    fn load_of_a_missing_folder_is_empty() {
        let scratch = Scratch::new("missing");
        assert_eq!(load(&scratch.folder()), []);
    }
}
