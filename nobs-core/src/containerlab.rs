use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub struct Containerlab {
    topology: PathBuf,
    sudo: bool,
    names: Vec<String>,
}

impl Containerlab {
    pub const PROGRAM: &'static str = "containerlab";
    pub const SUDO: &'static str = "sudo";

    pub fn new(topology: impl Into<PathBuf>) -> Self {
        Self {
            topology: topology.into(),
            sudo: false,
            names: Vec::new(),
        }
    }

    pub fn topology(&self) -> &Path {
        &self.topology
    }

    pub fn sudo(mut self, on: bool) -> Self {
        self.sudo = on;
        self
    }

    pub fn preserve_env(mut self, names: Vec<String>) -> Self {
        self.names = names;
        self
    }

    pub fn program(&self) -> &'static str {
        if self.sudo {
            Self::SUDO
        } else {
            Self::PROGRAM
        }
    }

    pub fn deploy_args(&self) -> Vec<OsString> {
        self.args("deploy", &[])
    }

    pub fn destroy_args(&self) -> Vec<OsString> {
        self.args("destroy", &["--cleanup"])
    }

    pub fn inspect_args(&self) -> Vec<OsString> {
        self.args("inspect", &[])
    }

    fn args(&self, action: &str, after_file: &[&str]) -> Vec<OsString> {
        let mut args = Vec::new();
        if self.sudo {
            if !self.names.is_empty() {
                args.push(format!("--preserve-env={}", self.names.join(",")).into());
            }
            args.push(Self::PROGRAM.into());
        }
        args.push(action.into());
        args.push("-t".into());
        args.push(self.topology.clone().into_os_string());
        args.extend(after_file.iter().map(OsString::from));
        args
    }
}

pub fn env_names(topology_text: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for (index, _) in topology_text.match_indices("${") {
        let rest = &topology_text[index + 2..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let name = &rest[..end];
        if name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            names.insert(name.to_string());
        }
    }
    names.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "lab/lab.clab.yaml";

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn without_sudo_the_program_is_containerlab_and_the_lists_are_plain() {
        let containerlab = Containerlab::new(FILE);
        assert_eq!(containerlab.program(), "containerlab");
        assert_eq!(containerlab.deploy_args(), ["deploy", "-t", FILE]);
        assert_eq!(
            containerlab.destroy_args(),
            ["destroy", "-t", FILE, "--cleanup"]
        );
        assert_eq!(containerlab.inspect_args(), ["inspect", "-t", FILE]);
    }

    #[test]
    fn without_sudo_the_names_are_not_used() {
        let containerlab = Containerlab::new(FILE).preserve_env(names(&["A"]));
        assert_eq!(containerlab.deploy_args(), ["deploy", "-t", FILE]);
    }

    #[test]
    fn with_sudo_the_program_is_sudo() {
        assert_eq!(Containerlab::new(FILE).sudo(true).program(), "sudo");
    }

    #[test]
    fn with_sudo_each_list_starts_with_preserve_env_then_containerlab() {
        let containerlab = Containerlab::new(FILE)
            .sudo(true)
            .preserve_env(names(&["NOBS_T_LABEL"]));
        assert_eq!(
            containerlab.deploy_args(),
            [
                "--preserve-env=NOBS_T_LABEL",
                "containerlab",
                "deploy",
                "-t",
                FILE
            ]
        );
        assert_eq!(
            containerlab.destroy_args(),
            [
                "--preserve-env=NOBS_T_LABEL",
                "containerlab",
                "destroy",
                "-t",
                FILE,
                "--cleanup"
            ]
        );
        assert_eq!(
            containerlab.inspect_args(),
            [
                "--preserve-env=NOBS_T_LABEL",
                "containerlab",
                "inspect",
                "-t",
                FILE
            ]
        );
    }

    #[test]
    fn with_sudo_two_names_are_joined_by_a_comma_in_one_argument() {
        let containerlab = Containerlab::new(FILE)
            .sudo(true)
            .preserve_env(names(&["A", "B"]));
        assert_eq!(containerlab.deploy_args()[0], "--preserve-env=A,B");
    }

    #[test]
    fn with_sudo_and_no_names_the_list_starts_with_containerlab() {
        let containerlab = Containerlab::new(FILE).sudo(true);
        assert_eq!(
            containerlab.deploy_args(),
            ["containerlab", "deploy", "-t", FILE]
        );
    }

    #[test]
    fn the_env_names_are_sorted_and_have_no_repeats() {
        assert_eq!(
            env_names("a: ${B:-x}\nb: ${A}\nc: ${B}\nd: $C\ne: ${}"),
            ["A", "B"]
        );
    }

    #[test]
    fn a_name_may_hold_digits_and_underscores_and_may_not_start_with_a_digit() {
        assert_eq!(env_names("${_A1-x} ${1A} ${A_B:?e}"), ["A_B", "_A1"]);
    }
}
