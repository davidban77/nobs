use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub struct Compose {
    file: PathBuf,
}

impl Compose {
    pub const PROGRAM: &'static str = "docker";

    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self { file: file.into() }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    pub fn ps_args(&self) -> Vec<OsString> {
        vec![
            "compose".into(),
            "-f".into(),
            self.file.clone().into_os_string(),
            "ps".into(),
            "--all".into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_runs_docker_compose_ps_all_on_the_file() {
        let compose = Compose::new("lab/docker-compose.yml");
        assert_eq!(Compose::PROGRAM, "docker");
        assert_eq!(
            compose.ps_args(),
            ["compose", "-f", "lab/docker-compose.yml", "ps", "--all"]
        );
    }
}
