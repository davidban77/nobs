use std::path::PathBuf;

use nobs_core::env::load_env;

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_value_already_in_the_process_wins_over_dot_env() {
    let dir = folder("shell-wins");
    std::env::set_var("NOBS_T_SHELL_WINS", "shell");
    std::fs::write(dir.join(".env"), "NOBS_T_SHELL_WINS=file\n").unwrap();

    load_env(&dir);

    assert_eq!(std::env::var("NOBS_T_SHELL_WINS").unwrap(), "shell");
}

#[test]
fn dot_env_wins_over_setup_env() {
    let dir = folder("env-wins");
    std::fs::write(dir.join(".env"), "NOBS_T_ENV_WINS=env\n").unwrap();
    std::fs::write(dir.join(".setup.env"), "NOBS_T_ENV_WINS=setup\n").unwrap();

    load_env(&dir);

    assert_eq!(std::env::var("NOBS_T_ENV_WINS").unwrap(), "env");
}

#[test]
fn a_key_only_in_setup_env_is_loaded() {
    let dir = folder("setup-only");
    std::fs::write(dir.join(".setup.env"), "NOBS_T_SETUP_ONLY=setup\n").unwrap();

    load_env(&dir);

    assert_eq!(std::env::var("NOBS_T_SETUP_ONLY").unwrap(), "setup");
}

#[test]
fn an_empty_folder_sets_nothing() {
    let dir = folder("empty");
    let before: Vec<_> = std::env::vars_os().collect();

    load_env(&dir);

    assert_eq!(std::env::vars_os().collect::<Vec<_>>(), before);
}
