use std::fs;
use std::path::{Path, PathBuf};

pub fn listing(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.push((path.clone(), String::new()));
            found.extend(listing(&path));
        } else {
            found.push((path.clone(), fs::read_to_string(&path).unwrap()));
        }
    }
    found.sort();
    found
}
