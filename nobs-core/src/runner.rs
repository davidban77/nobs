use std::cell::RefCell;
use std::ffi::OsString;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait Runner {
    fn run(&self, program: &str, args: &[OsString]) -> io::Result<i32>;
    fn output(&self, program: &str, args: &[OsString]) -> io::Result<Output>;
}

pub struct RealRunner;

fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

impl Runner for RealRunner {
    fn run(&self, program: &str, args: &[OsString]) -> io::Result<i32> {
        let status = Command::new(program).args(args).status()?;
        Ok(exit_code(status))
    }

    fn output(&self, program: &str, args: &[OsString]) -> io::Result<Output> {
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()?;
        Ok(Output {
            code: exit_code(output.status),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::new(),
        })
    }
}

pub struct CapturedRunner;

impl Runner for CapturedRunner {
    fn run(&self, program: &str, args: &[OsString]) -> io::Result<i32> {
        RealRunner.run(program, args)
    }

    fn output(&self, program: &str, args: &[OsString]) -> io::Result<Output> {
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()?;
        Ok(Output {
            code: exit_code(output.status),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub program: String,
    pub args: Vec<OsString>,
}

pub struct FakeRunner {
    exit_code: Option<i32>,
    answers: Vec<(String, String)>,
    default_answer: String,
    calls: RefCell<Vec<Call>>,
    outputs: RefCell<Vec<Call>>,
}

impl FakeRunner {
    pub fn returning(exit_code: i32) -> Self {
        Self {
            exit_code: Some(exit_code),
            answers: Vec::new(),
            default_answer: String::new(),
            calls: RefCell::new(Vec::new()),
            outputs: RefCell::new(Vec::new()),
        }
    }

    pub fn failing() -> Self {
        Self {
            exit_code: None,
            ..Self::returning(0)
        }
    }

    pub fn answering(stdout: impl Into<String>) -> Self {
        Self {
            default_answer: stdout.into(),
            ..Self::returning(0)
        }
    }

    pub fn answering_for(answers: &[(&str, &str)]) -> Self {
        Self {
            answers: answers
                .iter()
                .map(|(program, stdout)| (program.to_string(), stdout.to_string()))
                .collect(),
            ..Self::returning(0)
        }
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    pub fn outputs(&self) -> Vec<Call> {
        self.outputs.borrow().clone()
    }
}

impl Runner for FakeRunner {
    fn run(&self, program: &str, args: &[OsString]) -> io::Result<i32> {
        self.calls.borrow_mut().push(Call {
            program: program.to_string(),
            args: args.to_vec(),
        });
        self.exit_code
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "fake runner failure"))
    }

    fn output(&self, program: &str, args: &[OsString]) -> io::Result<Output> {
        self.outputs.borrow_mut().push(Call {
            program: program.to_string(),
            args: args.to_vec(),
        });
        let code = self
            .exit_code
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "fake runner failure"))?;
        let stdout = self
            .answers
            .iter()
            .find(|(name, _)| name == program)
            .map_or(&self.default_answer, |(_, stdout)| stdout);
        Ok(Output {
            code,
            stdout: stdout.clone(),
            stderr: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn real_runner_returns_zero_for_a_succeeding_program() {
        assert_eq!(RealRunner.run("true", &[]).unwrap(), 0);
    }

    #[test]
    fn real_runner_returns_one_for_a_failing_program() {
        assert_eq!(RealRunner.run("false", &[]).unwrap(), 1);
    }

    #[test]
    fn real_runner_returns_the_exit_code_the_child_chose() {
        assert_eq!(RealRunner.run("sh", &args(&["-c", "exit 7"])).unwrap(), 7);
    }

    #[test]
    fn real_runner_passes_arguments_without_a_shell() {
        let code = RealRunner
            .run("sh", &args(&["-c", "test \"$0\" = 'a b; c'", "a b; c"]))
            .unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn real_runner_reports_a_killed_child_as_128_plus_signal() {
        assert_eq!(
            RealRunner.run("sh", &args(&["-c", "kill -9 $$"])).unwrap(),
            137
        );
    }

    #[test]
    fn real_runner_fails_for_a_missing_program() {
        assert!(RealRunner.run("nobs-no-such-program", &[]).is_err());
    }

    #[test]
    fn fake_runner_records_every_call_and_returns_its_exit_code() {
        let runner = FakeRunner::returning(3);
        assert_eq!(runner.run("docker", &args(&["ps"])).unwrap(), 3);
        assert_eq!(runner.run("docker", &args(&["info"])).unwrap(), 3);
        assert_eq!(
            runner.calls(),
            vec![
                Call {
                    program: "docker".to_string(),
                    args: args(&["ps"])
                },
                Call {
                    program: "docker".to_string(),
                    args: args(&["info"])
                },
            ]
        );
    }

    #[test]
    fn real_runner_output_captures_stdout_and_the_exit_code() {
        let output = RealRunner
            .output("sh", &args(&["-c", "printf hi; exit 3"]))
            .unwrap();
        assert_eq!(
            output,
            Output {
                code: 3,
                stdout: "hi".to_string(),
                stderr: String::new()
            }
        );
    }

    #[test]
    fn captured_runner_output_captures_stdout_stderr_and_the_exit_code() {
        let output = CapturedRunner
            .output("sh", &args(&["-c", "printf out; printf err >&2; exit 3"]))
            .unwrap();
        assert_eq!(
            output,
            Output {
                code: 3,
                stdout: "out".to_string(),
                stderr: "err".to_string()
            }
        );
    }

    #[test]
    fn real_runner_output_leaves_stderr_to_the_terminal_and_returns_it_empty() {
        let output = RealRunner
            .output("sh", &args(&["-c", "printf out; printf err >&2; exit 3"]))
            .unwrap();
        assert_eq!(output.stderr, "");
    }

    #[test]
    fn real_runner_output_reports_a_killed_child_as_128_plus_signal() {
        let output = RealRunner
            .output("sh", &args(&["-c", "kill -9 $$"]))
            .unwrap();
        assert_eq!(output.code, 137);
    }

    #[test]
    fn an_answering_fake_returns_its_text_and_keeps_output_calls_apart() {
        let runner = FakeRunner::answering("a\nb\n");
        let output = runner.output("docker", &args(&["ps"])).unwrap();
        assert_eq!(
            output,
            Output {
                code: 0,
                stdout: "a\nb\n".to_string(),
                stderr: String::new()
            }
        );
        let asked = Call {
            program: "docker".to_string(),
            args: args(&["ps"]),
        };
        assert_eq!(runner.outputs(), vec![asked.clone()]);
        assert!(runner.calls().is_empty());

        assert_eq!(runner.run("docker", &args(&["info"])).unwrap(), 0);
        assert_eq!(runner.outputs(), vec![asked]);
        assert_eq!(
            runner.calls(),
            vec![Call {
                program: "docker".to_string(),
                args: args(&["info"])
            }]
        );
    }

    #[test]
    fn answering_for_gives_each_program_its_own_text() {
        let runner = FakeRunner::answering_for(&[("sysctl", "1\n"), ("df", "2\n")]);
        assert_eq!(runner.output("df", &[]).unwrap().stdout, "2\n");
        assert_eq!(runner.output("sysctl", &[]).unwrap().stdout, "1\n");
        let other = runner.output("docker", &[]).unwrap();
        assert_eq!(
            other,
            Output {
                code: 0,
                stdout: String::new(),
                stderr: String::new()
            }
        );
        assert_eq!(runner.run("df", &[]).unwrap(), 0);
    }

    #[test]
    fn output_follows_returning_and_failing() {
        assert!(FakeRunner::failing().output("docker", &[]).is_err());
        assert_eq!(
            FakeRunner::returning(3).output("docker", &[]).unwrap(),
            Output {
                code: 3,
                stdout: String::new(),
                stderr: String::new()
            }
        );
    }
}
