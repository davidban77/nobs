use std::ffi::OsString;
use std::fs;
use std::path::Path;

use crate::runner::Runner;

pub const URLS: [(&str, &str); 7] = [
    ("Grafana", "http://localhost:3000"),
    ("Prometheus", "http://localhost:9090"),
    ("Alertmanager", "http://localhost:9093"),
    ("Loki", "http://localhost:3001"),
    ("Infrahub", "http://localhost:8000"),
    ("Prefect", "http://localhost:4200"),
    ("Sonda", "http://localhost:8085"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bootstrap {
    Copied,
    Kept,
    NoExample,
}

pub fn bootstrap(dir: &Path) -> Result<Bootstrap, String> {
    let env = dir.join(".env");
    if env.exists() {
        return Ok(Bootstrap::Kept);
    }
    let example = dir.join(".env.example");
    if !example.exists() {
        return Ok(Bootstrap::NoExample);
    }
    fs::copy(&example, &env).map_err(|error| format!("{}: {error}", env.display()))?;
    Ok(Bootstrap::Copied)
}

pub fn foreign_containers(runner: &dyn Runner, project: &str) -> Result<Vec<String>, String> {
    let args: Vec<OsString> = vec![
        "ps".into(),
        "-a".into(),
        "--filter".into(),
        format!("label=com.docker.compose.project={project}").into(),
        "--format".into(),
        "{{.Names}}".into(),
    ];
    let output = runner
        .output("docker", &args)
        .map_err(|error| format!("cannot run docker: {error}"))?;
    if output.code != 0 {
        return Ok(Vec::new());
    }
    Ok(output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{Call, FakeRunner};
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("nobs-{}-{test}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn bootstrap_copies_the_example_when_there_is_no_env() {
        let temp = TempDir::new("bootstrap_copies");
        fs::write(temp.0.join(".env.example"), b"A=1\n\xff\n").unwrap();
        assert_eq!(bootstrap(&temp.0), Ok(Bootstrap::Copied));
        assert_eq!(fs::read(temp.0.join(".env")).unwrap(), b"A=1\n\xff\n");
    }

    #[test]
    fn bootstrap_keeps_an_existing_env_untouched() {
        let temp = TempDir::new("bootstrap_keeps");
        fs::write(temp.0.join(".env.example"), b"A=1\n").unwrap();
        fs::write(temp.0.join(".env"), b"A=2\n").unwrap();
        assert_eq!(bootstrap(&temp.0), Ok(Bootstrap::Kept));
        assert_eq!(fs::read(temp.0.join(".env")).unwrap(), b"A=2\n");
    }

    #[test]
    fn bootstrap_with_neither_file_reports_no_example() {
        let temp = TempDir::new("bootstrap_neither");
        assert_eq!(bootstrap(&temp.0), Ok(Bootstrap::NoExample));
        assert!(!temp.0.join(".env").exists());
    }

    #[test]
    fn bootstrap_names_the_env_path_when_the_copy_fails() {
        let temp = TempDir::new("bootstrap_fails");
        fs::create_dir(temp.0.join(".env.example")).unwrap();
        let error = bootstrap(&temp.0).unwrap_err();
        let prefix = format!("{}: ", temp.0.join(".env").display());
        assert!(error.starts_with(&prefix), "{error}");
    }

    #[test]
    fn foreign_containers_asks_docker_ps_for_the_project_label() {
        let runner = FakeRunner::answering("sonda-server\nloki\n");
        assert_eq!(
            foreign_containers(&runner, "packt"),
            Ok(vec!["sonda-server".to_string(), "loki".to_string()])
        );
        let expected: Vec<OsString> = [
            "ps",
            "-a",
            "--filter",
            "label=com.docker.compose.project=packt",
            "--format",
            "{{.Names}}",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        assert_eq!(
            runner.outputs(),
            vec![Call {
                program: "docker".to_string(),
                env: Vec::new(),
                args: expected
            }]
        );
    }

    #[test]
    fn foreign_containers_skips_blank_lines_and_trims_names() {
        let runner = FakeRunner::answering(" a \n\n  \nb\n");
        assert_eq!(
            foreign_containers(&runner, "p"),
            Ok(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn foreign_containers_is_empty_for_empty_text() {
        assert_eq!(
            foreign_containers(&FakeRunner::answering(""), "p"),
            Ok(Vec::new())
        );
    }

    #[test]
    fn foreign_containers_fails_open_on_a_non_zero_code() {
        assert_eq!(
            foreign_containers(&FakeRunner::returning(3), "p"),
            Ok(Vec::new())
        );
    }

    #[test]
    fn foreign_containers_reports_a_docker_that_cannot_start() {
        let error = foreign_containers(&FakeRunner::failing(), "p").unwrap_err();
        assert!(error.starts_with("cannot run docker: "), "{error}");
    }
}
