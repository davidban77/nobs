use std::cell::RefCell;
use std::ffi::OsString;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
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
    script: Option<Vec<(i32, String)>>,
    calls: RefCell<Vec<Call>>,
    outputs: RefCell<Vec<Call>>,
    log: RefCell<Vec<Call>>,
}

impl FakeRunner {
    pub fn returning(exit_code: i32) -> Self {
        Self {
            exit_code: Some(exit_code),
            answers: Vec::new(),
            default_answer: String::new(),
            script: None,
            calls: RefCell::new(Vec::new()),
            outputs: RefCell::new(Vec::new()),
            log: RefCell::new(Vec::new()),
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

    pub fn scripted(steps: &[(i32, &str)]) -> Self {
        Self {
            script: Some(
                steps
                    .iter()
                    .map(|(code, stdout)| (*code, stdout.to_string()))
                    .collect(),
            ),
            ..Self::returning(0)
        }
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    pub fn outputs(&self) -> Vec<Call> {
        self.outputs.borrow().clone()
    }

    pub fn all_calls(&self) -> Vec<Call> {
        self.log.borrow().clone()
    }

    fn record(&self, list: &RefCell<Vec<Call>>, program: &str, args: &[OsString]) -> usize {
        let call = Call {
            program: program.to_string(),
            args: args.to_vec(),
        };
        let mut log = self.log.borrow_mut();
        let index = log.len();
        log.push(call.clone());
        list.borrow_mut().push(call);
        index
    }

    fn step(&self, index: usize) -> Option<(i32, &str)> {
        let script = self.script.as_ref()?;
        Some(
            script
                .get(index)
                .map_or((0, ""), |(code, stdout)| (*code, stdout.as_str())),
        )
    }
}

impl Runner for FakeRunner {
    fn run(&self, program: &str, args: &[OsString]) -> io::Result<i32> {
        let index = self.record(&self.calls, program, args);
        if let Some((code, _)) = self.step(index) {
            return Ok(code);
        }
        self.exit_code
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "fake runner failure"))
    }

    fn output(&self, program: &str, args: &[OsString]) -> io::Result<Output> {
        let index = self.record(&self.outputs, program, args);
        if let Some((code, stdout)) = self.step(index) {
            return Ok(Output {
                code,
                stdout: stdout.to_string(),
            });
        }
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
                stdout: "hi".to_string()
            }
        );
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
                stdout: "a\nb\n".to_string()
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
                stdout: String::new()
            }
        );
        assert_eq!(runner.run("df", &[]).unwrap(), 0);
    }

    #[test]
    fn a_scripted_fake_answers_the_nth_call_of_either_kind_with_the_nth_step() {
        let runner = FakeRunner::scripted(&[(3, "a"), (0, "b"), (5, "c")]);
        assert_eq!(runner.run("docker", &[]).unwrap(), 3);
        assert_eq!(
            runner.output("docker", &[]).unwrap(),
            Output {
                code: 0,
                stdout: "b".to_string()
            }
        );
        assert_eq!(
            runner.output("docker", &[]).unwrap(),
            Output {
                code: 5,
                stdout: "c".to_string()
            }
        );
        assert_eq!(runner.run("docker", &[]).unwrap(), 0);
        assert_eq!(
            runner.output("docker", &[]).unwrap(),
            Output {
                code: 0,
                stdout: String::new()
            }
        );
    }

    #[test]
    fn all_calls_keeps_the_order_across_run_and_output() {
        let runner = FakeRunner::returning(0);
        runner.run("a", &args(&["1"])).unwrap();
        runner.output("b", &args(&["2"])).unwrap();
        runner.run("c", &args(&["3"])).unwrap();
        let programs: Vec<String> = runner
            .all_calls()
            .into_iter()
            .map(|call| call.program)
            .collect();
        assert_eq!(programs, ["a", "b", "c"]);
        assert_eq!(runner.calls().len(), 2);
        assert_eq!(runner.outputs().len(), 1);
    }

    #[test]
    fn output_follows_returning_and_failing() {
        assert!(FakeRunner::failing().output("docker", &[]).is_err());
        assert_eq!(
            FakeRunner::returning(3).output("docker", &[]).unwrap(),
            Output {
                code: 3,
                stdout: String::new()
            }
        );
    }
}
