use std::fs;
use std::path::PathBuf;

pub fn product_code() -> Vec<(PathBuf, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for crate_dir in fs::read_dir(&root).unwrap().flatten() {
        let source = crate_dir.path().join("src");
        if !crate_dir.file_name().to_string_lossy().starts_with("nobs-") || !source.is_dir() {
            continue;
        }
        for file in fs::read_dir(&source).unwrap().flatten() {
            let path = file.path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                let text = fs::read_to_string(&path).unwrap();
                let product = text.split("mod tests {").next().unwrap().to_string();
                files.push((path, product));
            }
        }
    }
    files
}
