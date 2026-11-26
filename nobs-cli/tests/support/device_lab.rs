use std::fs;
use std::path::PathBuf;
use std::process::Output;

pub const TEST_PASSWORD: &str = "nobs-t-not-a-secret";

pub struct Lab(pub PathBuf);

impl Lab {
    pub fn new(name: &str) -> Lab {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("cli-{}-{name}", env!("CARGO_CRATE_NAME")));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Lab(root.canonicalize().unwrap())
    }

    pub fn write(&self, relative: &str, text: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn assert_fails_with(output: &Output, line: &str) {
    let stderr = stderr(output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr, format!("{line}\n"));
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!stderr.contains(TEST_PASSWORD));
}
