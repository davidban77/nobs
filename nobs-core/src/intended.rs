use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use crate::device::{Reading, Words};
use crate::render::{Renderer, TEMPLATES};
use crate::sot::{Error, SourceOfTruth};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    NoTemplate(String),
    Other(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::NoTemplate(text) | Failure::Other(text) => f.write_str(text),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked<'a> {
    pub line: &'a str,
    pub lacking: Option<Vec<&'a str>>,
}

pub fn intended(
    lab: &Path,
    name: &str,
    kind: &str,
    source: Result<&dyn SourceOfTruth, String>,
) -> Result<String, Failure> {
    let renderer = Renderer::new(&lab.join(TEMPLATES));
    let file = renderer.template(kind).map_err(Failure::NoTemplate)?;
    let vars = source
        .map_err(Failure::Other)?
        .node_vars(name)
        .map_err(|error| Failure::Other(error.to_string()))?
        .ok_or_else(|| {
            Failure::Other(
                Error::NotFound {
                    kind: "device".to_string(),
                    device: name.to_string(),
                }
                .to_string(),
            )
        })?;
    renderer
        .config(&file, name, kind, &vars)
        .map_err(Failure::Other)
}

pub fn lines(intended: &str) -> impl Iterator<Item = &str> {
    intended
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
}

pub fn compare<'a>(
    intended: &'a str,
    fetched: &'a [String],
    reading: Option<Reading>,
) -> Vec<Checked<'a>> {
    let words =
        |line: &'a str| reading.map_or(Words::Unread(line.trim()), |reading| reading.words(line));
    let device: Vec<(&str, Words)> = fetched
        .iter()
        .map(|line| (line.as_str(), words(line)))
        .collect();
    let present: HashSet<&Words> = device.iter().map(|(_, read)| read).collect();
    let stated: HashSet<Words> = lines(intended).map(words).collect();
    intended
        .lines()
        .map(|line| {
            let wanted = words(line);
            let lacking = (!line.trim().is_empty() && !present.contains(&wanted)).then(|| {
                device
                    .iter()
                    .filter(|(_, candidate)| {
                        !stated.contains(candidate)
                            && head(candidate).is_some()
                            && head(candidate) == head(&wanted)
                    })
                    .map(|(candidate, _)| *candidate)
                    .collect()
            });
            Checked { line, lacking }
        })
        .collect()
}

pub fn missing(checked: &[Checked<'_>]) -> usize {
    checked
        .iter()
        .filter(|checked| checked.lacking.is_some())
        .count()
}

fn head<'w, 'a>(words: &'w Words<'a>) -> Option<&'w [Cow<'a, str>]> {
    match words {
        Words::Read(words) => words.split_last().map(|(_, head)| head),
        Words::Unread(_) => None,
    }
    .filter(|head| !head.is_empty())
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
            intended(&lab.0, "nobs-t-srl1", "nokia_srlinux", Ok(&source)),
            Ok(SRL1.to_string())
        );
        assert_eq!(
            intended(&lab.0, "nobs-t-srl2", "nokia_srlinux", Ok(&source)),
            Ok(SRL2.to_string())
        );
    }

    #[test]
    fn a_node_the_source_does_not_know_is_not_found() {
        let lab = Lab::new("unknown", Some(SRL_TEMPLATE));
        assert_eq!(
            intended(&lab.0, "nobs-t-srl9", "nokia_srlinux", Ok(&fixed())),
            Err(Failure::Other(
                "device nobs-t-srl9 not found in the source of truth.".to_string()
            ))
        );
    }

    #[test]
    fn infrahub_gives_no_data_for_templates() {
        let lab = Lab::new("infrahub", Some(SRL_TEMPLATE));
        let source = Infrahub::new("http://nobs-t-infrahub:8000", "");
        assert_eq!(
            intended(&lab.0, "nobs-t-srl1", "nokia_srlinux", Ok(&source)),
            Err(Failure::Other(
                "the source of truth gives no data for templates".to_string()
            ))
        );
    }

    #[test]
    fn a_kind_without_a_template_is_said_before_the_source_is_asked() {
        let lab = Lab::new("no-template", None);
        let expected = Err(Failure::NoTemplate(
            "no template for kind nokia_srlinux: expected templates/srl.j2".to_string(),
        ));
        assert_eq!(
            intended(&lab.0, "nobs-t-srl9", "nokia_srlinux", Ok(&fixed())),
            expected
        );
        let infrahub = Infrahub::new("http://nobs-t-infrahub:8000", "");
        assert_eq!(
            intended(&lab.0, "nobs-t-srl1", "nokia_srlinux", Ok(&infrahub)),
            expected
        );
    }

    #[test]
    fn a_template_file_that_does_not_read_and_a_source_error_are_other_failures() {
        let lab = Lab::new("unreadable", None);
        fs::write(lab.0.join(TEMPLATES).join("srl.j2"), [0xff, 0xfe, 0xfd]).unwrap();
        let Err(Failure::Other(text)) =
            intended(&lab.0, "nobs-t-srl1", "nokia_srlinux", Ok(&fixed()))
        else {
            panic!("a template that does not read is not an other failure");
        };
        assert!(text.starts_with("templates/srl.j2: "), "{text}");

        let lab = Lab::new("source-error", Some(SRL_TEMPLATE));
        assert_eq!(
            intended(
                &lab.0,
                "nobs-t-srl1",
                "nokia_srlinux",
                Err("nobs-t-source-error".to_string())
            ),
            Err(Failure::Other("nobs-t-source-error".to_string()))
        );
    }

    const SRL: Option<Reading> = Some(Reading::SrLinux);

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn checked<'a>(line: &'a str, lacking: Option<Vec<&'a str>>) -> Checked<'a> {
        Checked { line, lacking }
    }

    #[test]
    fn a_line_on_the_device_is_not_lacking() {
        let fetched = lines("set / a b on");
        assert_eq!(
            compare("set / a b on\n", &fetched, SRL),
            [checked("set / a b on", None)]
        );
    }

    #[test]
    fn white_space_at_both_ends_of_a_line_is_no_difference() {
        let fetched = vec!["set / a b on\r".to_string(), "\tset / c d on  ".to_string()];
        assert_eq!(
            compare("  set / a b on \nset / c d on", &fetched, SRL),
            [
                checked("  set / a b on ", None),
                checked("set / c d on", None)
            ]
        );
    }

    #[test]
    fn more_space_between_words_is_no_difference() {
        let fetched = lines("set / a b on");
        assert_eq!(
            compare("set / a  b on", &fetched, SRL),
            [checked("set / a  b on", None)]
        );
    }

    const TYPED: &str = "set / system information location \"nobs-t-x\"\nset / interface 'ethernet-1/1' description \"to spine\"\n";
    const PRINTED: &str = "set / system information location nobs-t-x\nset / interface ethernet-1/1 description \"to spine\"";

    #[test]
    fn a_line_the_device_prints_in_its_own_form_is_not_lacking_either_way() {
        let printed = lines(PRINTED);
        assert_eq!(missing(&compare(TYPED, &printed, SRL)), 0);
        let typed = lines(TYPED);
        assert_eq!(missing(&compare(PRINTED, &typed, SRL)), 0);
    }

    #[test]
    fn a_quoted_word_inside_a_line_still_finds_the_device_line_as_its_hint() {
        let fetched = lines("set / interface ethernet-1/1 description uplink");
        let intended = "set / interface \"ethernet-1/1\" description \"to spine\"";
        assert_eq!(
            compare(intended, &fetched, SRL),
            [checked(
                intended,
                Some(vec!["set / interface ethernet-1/1 description uplink"])
            )]
        );
    }

    #[test]
    fn a_device_line_the_template_states_in_another_form_is_never_a_hint() {
        let fetched = lines("set / v 10\nset / v 30");
        assert_eq!(
            compare("set / v 20\nset / v \"30\"", &fetched, SRL),
            [
                checked("set / v 20", Some(vec!["set / v 10"])),
                checked("set / v \"30\"", None)
            ]
        );
    }

    #[test]
    fn an_unread_line_is_the_same_only_as_its_own_text_and_has_no_hints() {
        let fetched = lines("set / d \"a\nset / d a\nset / d b");
        assert_eq!(
            compare("set / d \"a\nset / d \"b", &fetched, SRL),
            [
                checked("set / d \"a", None),
                checked("set / d \"b", Some(vec![]))
            ]
        );
    }

    #[test]
    fn with_no_reading_a_line_is_compared_as_its_text() {
        let fetched = lines("x a");
        assert_eq!(
            compare("x \"a\"", &fetched, None),
            [checked("x \"a\"", Some(vec![]))]
        );
        assert_eq!(
            compare("x \"a\"", &fetched, SRL),
            [checked("x \"a\"", None)]
        );
    }

    #[test]
    fn a_lacking_line_with_no_device_line_of_the_same_start_has_no_hints() {
        let fetched = lines("set / a d off\nset / a c\nset / x c on");
        assert_eq!(
            compare("set / a c on", &fetched, SRL),
            [checked("set / a c on", Some(vec![]))]
        );
    }

    #[test]
    fn a_changed_value_gives_the_device_line_as_its_only_hint() {
        let fetched = lines("set / a b off\nset / z y on");
        assert_eq!(
            compare("set / a b on\nset / z y on", &fetched, SRL),
            [
                checked("set / a b on", Some(vec!["set / a b off"])),
                checked("set / z y on", None)
            ]
        );
    }

    #[test]
    fn every_device_value_of_the_same_start_is_a_hint_in_the_device_order() {
        let fetched = lines("set / v 30\nset / w 1\nset / v 10");
        assert_eq!(
            compare("set / v 20", &fetched, SRL),
            [checked(
                "set / v 20",
                Some(vec!["set / v 30", "set / v 10"])
            )]
        );
    }

    #[test]
    fn a_device_line_the_intended_text_also_states_is_never_a_hint() {
        let fetched = lines("set / v 10\nset / v 30");
        assert_eq!(
            compare("set / v 20\n  set / v 30", &fetched, SRL),
            [
                checked("set / v 20", Some(vec!["set / v 10"])),
                checked("  set / v 30", None)
            ]
        );
    }

    #[test]
    fn a_line_of_one_word_has_no_hints() {
        let fetched = lines("save\n  commit now");
        assert_eq!(
            compare("commit", &fetched, SRL),
            [checked("commit", Some(vec![]))]
        );
    }

    #[test]
    fn a_value_in_double_quotes_is_one_word() {
        let fetched = lines("set / d \"uplink\"\nset / d \"to spine\" 2\nset / d \"to\"");
        assert_eq!(
            compare("set / d \"to spine 1\"", &fetched, SRL),
            [checked(
                "set / d \"to spine 1\"",
                Some(vec!["set / d \"uplink\"", "set / d \"to\""])
            )]
        );
    }

    #[test]
    fn a_lacking_line_written_twice_is_lacking_twice() {
        let fetched = lines("set / a b on");
        assert_eq!(
            compare(
                "set / a b on\nset / c d on\nset / a b on\nset / c d on",
                &fetched,
                SRL
            ),
            [
                checked("set / a b on", None),
                checked("set / c d on", Some(vec![])),
                checked("set / a b on", None),
                checked("set / c d on", Some(vec![]))
            ]
        );
    }

    #[test]
    fn an_empty_or_blank_intended_line_is_never_lacking() {
        let fetched = lines("set / a b on");
        assert_eq!(
            compare("\n  \t\nset / a b on\n", &fetched, SRL),
            [
                checked("", None),
                checked("  \t", None),
                checked("set / a b on", None)
            ]
        );
    }

    #[test]
    fn each_line_of_a_value_that_goes_on_in_further_lines_is_compared_alone() {
        let fetched = lines("-----BEGIN X-----\nnobs-t-body\n-----END X-----");
        assert_eq!(
            compare(
                "-----BEGIN X-----\nnobs-t-other\n-----END X-----",
                &fetched,
                SRL
            ),
            [
                checked("-----BEGIN X-----", None),
                checked("nobs-t-other", Some(vec![])),
                checked("-----END X-----", None)
            ]
        );
    }

    #[test]
    fn missing_counts_each_lacking_line_with_or_without_hints_and_no_present_or_blank_one() {
        let fetched = lines("set / a b off\nset / c d on");
        assert_eq!(missing(&compare("set / c d on\n\n  \t", &fetched, SRL)), 0);
        assert_eq!(
            missing(&compare("set / a b on\nset / c d on\n", &fetched, SRL)),
            1
        );
        assert_eq!(
            missing(&compare(
                "set / a b on\nset / x y on\nset / c d on\nset / a b on",
                &fetched,
                SRL
            )),
            3
        );
    }

    #[test]
    fn a_device_line_the_template_never_states_is_never_in_the_result() {
        let fetched = lines("set / x y on\nset / a b on\nset / a q on\nset / z");
        assert_eq!(
            compare("set / a b on", &fetched, SRL),
            [checked("set / a b on", None)]
        );
    }
}
