use std::cell::RefCell;
use std::ffi::OsString;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;

pub trait Runner {
    fn run(&self, program: &str, args: &[OsString]) -> io::Result<i32>;
}

pub struct RealRunner;

impl Runner for RealRunner {
    fn run(&self, program: &str, args: &[OsString]) -> io::Result<i32> {
        let status = Command::new(program).args(args).status()?;
        Ok(status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub program: String,
    pub args: Vec<OsString>,
}

pub struct FakeRunner {
    exit_code: Option<i32>,
    calls: RefCell<Vec<Call>>,
}

impl FakeRunner {
    pub fn returning(exit_code: i32) -> Self {
        Self {
            exit_code: Some(exit_code),
            calls: RefCell::new(Vec::new()),
        }
    }

    pub fn failing() -> Self {
        Self {
            exit_code: None,
            calls: RefCell::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
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
}
