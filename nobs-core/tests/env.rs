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

    load_env(&dir).unwrap();

    assert_eq!(std::env::var("NOBS_T_SHELL_WINS").unwrap(), "shell");
}

#[test]
fn dot_env_wins_over_setup_env() {
    let dir = folder("env-wins");
    std::fs::write(dir.join(".env"), "NOBS_T_ENV_WINS=env\n").unwrap();
    std::fs::write(dir.join(".setup.env"), "NOBS_T_ENV_WINS=setup\n").unwrap();

    load_env(&dir).unwrap();

    assert_eq!(std::env::var("NOBS_T_ENV_WINS").unwrap(), "env");
}

#[test]
fn a_key_only_in_setup_env_is_loaded() {
    let dir = folder("setup-only");
    std::fs::write(dir.join(".setup.env"), "NOBS_T_SETUP_ONLY=setup\n").unwrap();

    load_env(&dir).unwrap();

    assert_eq!(std::env::var("NOBS_T_SETUP_ONLY").unwrap(), "setup");
}

#[test]
fn an_empty_folder_sets_nothing() {
    let dir = folder("empty");
    let before: Vec<_> = std::env::vars_os().collect();

    load_env(&dir).unwrap();

    assert_eq!(std::env::vars_os().collect::<Vec<_>>(), before);
}

#[test]
fn an_empty_key_is_an_error_naming_the_file_and_line_and_sets_nothing() {
    let dir = folder("empty-key");
    std::fs::write(dir.join(".env"), "=x\n").unwrap();
    let before: Vec<_> = std::env::vars_os().collect();

    let error = load_env(&dir).unwrap_err();

    assert!(error.contains(".env"), "{error}");
    assert!(error.contains("line 1"), "{error}");
    assert_eq!(std::env::vars_os().collect::<Vec<_>>(), before);
}

#[test]
fn a_nul_byte_in_a_key_or_a_value_is_an_error_naming_the_line_and_sets_nothing() {
    let key_dir = folder("nul-key");
    std::fs::write(key_dir.join(".env"), "# c\nNOBS_T_NUL\0KEY=x\n").unwrap();
    let value_dir = folder("nul-value");
    std::fs::write(value_dir.join(".env"), "\n# c\nNOBS_T_NUL_VALUE=a\0b\n").unwrap();
    let before: Vec<_> = std::env::vars_os().collect();

    let key_error = load_env(&key_dir).unwrap_err();
    let value_error = load_env(&value_dir).unwrap_err();

    assert!(key_error.contains("NUL byte"), "{key_error}");
    assert!(key_error.contains("line 2"), "{key_error}");
    assert!(value_error.contains("NUL byte"), "{value_error}");
    assert!(value_error.contains("line 3"), "{value_error}");
    assert_eq!(std::env::vars_os().collect::<Vec<_>>(), before);
}
