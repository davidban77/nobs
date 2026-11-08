use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::lab;
use crate::settings::{read_settings, read_settings_file, Lab, Settings, Workshop};

pub const FILE: &str = "nobs.toml";

#[derive(Debug, Clone, PartialEq)]
pub enum LocateError {
    NotFound,
    NotAt(PathBuf),
    Settings { path: PathBuf, reason: String },
}

impl fmt::Display for LocateError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "no lab or workshop found from here"),
            Self::NotAt(path) => write!(f, "no lab or workshop at {}", path.display()),
            Self::Settings { path, reason } => write!(f, "{}: {reason}", path.display()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Workshop(Workshop),
    BookLab(Option<Lab>),
    LabcliLab(Option<Lab>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    pub dir: PathBuf,
    pub kind: Kind,
}

impl Located {
    pub fn compose(&self, scenario: Option<&str>) -> PathBuf {
        if let Some(name) = scenario {
            return self
                .dir
                .join("chapters")
                .join(name)
                .join("docker-compose.yml");
        }
        match self.lab().and_then(|lab| lab.compose.as_ref()) {
            Some(file) => self.dir.join(file),
            None => self.dir.join("docker-compose.yml"),
        }
    }

    pub fn topology(&self) -> PathBuf {
        if let Some(file) = self.lab().and_then(|lab| lab.topology.as_ref()) {
            return self.dir.join(file);
        }
        let folder = self.dir.join("containerlab");
        if matches!(self.kind, Kind::BookLab(_)) {
            return folder.join("lab.yml");
        }
        let yaml = folder.join("lab.clab.yaml");
        if yaml.is_file() {
            yaml
        } else {
            folder.join("lab.clab.yml")
        }
    }

    pub fn project(&self) -> Option<&str> {
        if let Kind::Workshop(workshop) = &self.kind {
            return Some(&workshop.project);
        }
        let setting = self.lab().and_then(|lab| lab.project.as_deref());
        setting.or(self.book_default(lab::PROJECT))
    }

    pub fn network(&self) -> Option<&str> {
        let setting = self.lab().and_then(|lab| lab.network.as_deref());
        setting.or(self.book_default(lab::NETWORK))
    }

    fn lab(&self) -> Option<&Lab> {
        match &self.kind {
            Kind::BookLab(lab) | Kind::LabcliLab(lab) => lab.as_ref(),
            Kind::Workshop(_) => None,
        }
    }

    fn book_default(&self, name: &'static str) -> Option<&'static str> {
        matches!(self.kind, Kind::BookLab(_)).then_some(name)
    }
}

type Marker = fn(&Path) -> Result<Option<Kind>, LocateError>;

const MARKERS: [Marker; 4] = [
    settings_file,
    book_lab_layout,
    project_file,
    labcli_lab_layout,
];

fn settings_file(dir: &Path) -> Result<Option<Kind>, LocateError> {
    let file = dir.join(FILE);
    if !file.is_file() {
        return Ok(None);
    }
    let settings = fs::read_to_string(&file)
        .map_err(|error| error.to_string())
        .and_then(|text| read_settings(&text))
        .map_err(|reason| LocateError::Settings {
            path: file.clone(),
            reason,
        })?;
    Ok(Some(match settings {
        Settings::Workshop(workshop) => Kind::Workshop(workshop),
        Settings::Lab(lab) if has_book_layout(dir) => Kind::BookLab(Some(lab)),
        Settings::Lab(lab) => Kind::LabcliLab(Some(lab)),
    }))
}

fn has_book_layout(dir: &Path) -> bool {
    dir.join("chapters").is_dir() && dir.join("containerlab/lab.yml").is_file()
}

fn book_lab_layout(dir: &Path) -> Result<Option<Kind>, LocateError> {
    Ok(has_book_layout(dir).then_some(Kind::BookLab(None)))
}

fn project_file(dir: &Path) -> Result<Option<Kind>, LocateError> {
    Ok(dir
        .join("nobs_project.yml")
        .is_file()
        .then_some(Kind::LabcliLab(None)))
}

fn labcli_lab_layout(dir: &Path) -> Result<Option<Kind>, LocateError> {
    Ok(dir
        .join("docker-compose.yml")
        .is_file()
        .then_some(Kind::LabcliLab(None)))
}

pub fn locate(start: &Path, given: Option<&Path>) -> Result<Located, LocateError> {
    if let Some(dir) = given {
        for marker in MARKERS {
            if let Some(kind) = marker(dir)? {
                return Ok(Located {
                    dir: dir.to_path_buf(),
                    kind,
                });
            }
        }
        return Err(LocateError::NotAt(dir.to_path_buf()));
    }
    for marker in MARKERS {
        for dir in start.ancestors() {
            if let Some(kind) = marker(dir)? {
                return Ok(Located {
                    dir: dir.to_path_buf(),
                    kind,
                });
            }
        }
    }
    Err(LocateError::NotFound)
}

pub fn find_workshops(start: &Path) -> Vec<(String, String, PathBuf)> {
    let mut found = Vec::new();
    if let Ok(Located {
        dir,
        kind: Kind::Workshop(workshop),
    }) = locate(start, None)
    {
        found.push((workshop.name, workshop.title, dir));
    }
    for parent in [start.to_path_buf(), start.join("workshops")] {
        let Ok(entries) = fs::read_dir(&parent) else {
            continue;
        };
        for dir in entries.flatten().map(|entry| entry.path()) {
            if let Ok(Settings::Workshop(workshop)) = read_settings_file(&dir.join(FILE)) {
                found.push((workshop.name, workshop.title, dir));
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("nobs-locate-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let dir = self.0.join(relative);
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn file(&self, relative: &str, text: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }

        fn book_lab(&self, relative: &str) -> PathBuf {
            self.file(&format!("{relative}/chapters/ch1/docker-compose.yml"), "");
            self.file(&format!("{relative}/containerlab/lab.yml"), "");
            self.0.join(relative)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures")
            .join(relative)
    }

    fn workshop_text() -> String {
        fs::read_to_string(fixture("workshop-one/nobs.toml")).unwrap()
    }

    #[test]
    fn a_workshop_is_found_from_two_levels_below_it() {
        let scratch = Scratch::new("two-below");
        scratch.file("ws/nobs.toml", &workshop_text());
        let start = scratch.dir("ws/a/b");
        let located = locate(&start, None).unwrap();
        assert_eq!(located.dir, scratch.0.join("ws"));
        assert!(matches!(located.kind, Kind::Workshop(_)), "{located:?}");
    }

    #[test]
    fn a_book_lab_uses_the_chapter_compose_file_and_lab_yml() {
        let scratch = Scratch::new("book");
        let book = scratch.book_lab("book");
        let located = locate(&book, None).unwrap();
        assert_eq!(located.kind, Kind::BookLab(None));
        assert_eq!(
            located.compose(Some("ch1")),
            book.join("chapters/ch1/docker-compose.yml")
        );
        assert_eq!(located.topology(), book.join("containerlab/lab.yml"));
    }

    #[test]
    fn a_labcli_lab_prefers_lab_clab_yaml_and_falls_back_to_lab_clab_yml() {
        let scratch = Scratch::new("labcli");
        let lab = scratch.dir("lab");
        scratch.file("lab/docker-compose.yml", "");
        scratch.file("lab/containerlab/lab.clab.yml", "");
        let located = locate(&lab, None).unwrap();
        assert_eq!(located.kind, Kind::LabcliLab(None));
        assert_eq!(located.compose(None), lab.join("docker-compose.yml"));
        assert_eq!(located.topology(), lab.join("containerlab/lab.clab.yml"));
        scratch.file("lab/containerlab/lab.clab.yaml", "");
        assert_eq!(located.topology(), lab.join("containerlab/lab.clab.yaml"));
    }

    #[test]
    fn a_lab_settings_file_overrides_compose_topology_and_project() {
        let scratch = Scratch::new("overrides");
        let lab = scratch.dir("lab");
        scratch.file("lab/docker-compose.yml", "");
        scratch.file(
            "lab/nobs.toml",
            "kind = \"lab\"\ntopology = \"t/x.yml\"\ncompose = \"c/y.yml\"\nproject = \"p\"\n",
        );
        let located = locate(&lab, None).unwrap();
        assert!(
            matches!(located.kind, Kind::LabcliLab(Some(_))),
            "{located:?}"
        );
        assert_eq!(located.compose(None), lab.join("c/y.yml"));
        assert_eq!(located.topology(), lab.join("t/x.yml"));
        assert_eq!(located.project(), Some("p"));
    }

    #[test]
    fn a_nobs_toml_beats_a_book_lab_in_a_parent() {
        let scratch = Scratch::new("beats-book");
        let book = scratch.book_lab("book");
        scratch.file("book/inner/nobs.toml", &workshop_text());
        let located = locate(&book.join("inner"), None).unwrap();
        assert_eq!(located.dir, book.join("inner"));
        assert!(matches!(located.kind, Kind::Workshop(_)), "{located:?}");
    }

    #[test]
    fn the_walk_is_per_kind_so_a_chapter_compose_file_still_finds_the_book_lab() {
        let scratch = Scratch::new("per-kind");
        let book = scratch.book_lab("book");
        let located = locate(&book.join("chapters/ch1"), None).unwrap();
        assert_eq!(located.dir, book);
        assert_eq!(located.kind, Kind::BookLab(None));
    }

    #[test]
    fn a_given_matching_folder_wins_over_the_walk() {
        let scratch = Scratch::new("given");
        scratch.file("ws/nobs.toml", &workshop_text());
        let lab = scratch.dir("lab");
        scratch.file("lab/docker-compose.yml", "");
        let located = locate(&scratch.dir("ws/a"), Some(&lab)).unwrap();
        assert_eq!(located.dir, lab);
        assert_eq!(located.kind, Kind::LabcliLab(None));
    }

    #[test]
    fn a_given_bare_folder_is_an_error_naming_it() {
        let scratch = Scratch::new("given-bare");
        scratch.file("ws/nobs.toml", &workshop_text());
        let bare = scratch.dir("ws/bare");
        let error = locate(&bare, Some(&bare)).unwrap_err();
        assert_eq!(error, LocateError::NotAt(bare.clone()));
        assert_eq!(
            error.to_string(),
            format!("no lab or workshop at {}", bare.display())
        );
    }

    #[test]
    fn a_bare_folder_tree_finds_nothing() {
        let scratch = Scratch::new("bare");
        let error = locate(&scratch.dir("a/b"), None).unwrap_err();
        assert_eq!(error, LocateError::NotFound);
        assert_eq!(error.to_string(), "no lab or workshop found from here");
    }

    #[test]
    fn a_scenario_compose_file_is_under_chapters_for_every_kind() {
        let scratch = Scratch::new("scenario");
        let lab = scratch.dir("lab");
        scratch.file("lab/docker-compose.yml", "");
        let labcli = locate(&lab, None).unwrap();
        assert_eq!(
            labcli.compose(Some("x")),
            lab.join("chapters/x/docker-compose.yml")
        );
        scratch.file("lab/nobs.toml", "kind = \"lab\"\ncompose = \"c/y.yml\"\n");
        let overridden = locate(&lab, None).unwrap();
        assert!(
            matches!(overridden.kind, Kind::LabcliLab(Some(_))),
            "{overridden:?}"
        );
        assert_eq!(
            overridden.compose(Some("x")),
            lab.join("chapters/x/docker-compose.yml")
        );
    }

    #[test]
    fn project_is_the_workshop_project_netobs_for_a_book_lab_and_none_for_a_plain_lab() {
        let scratch = Scratch::new("project");
        let ws = scratch.dir("ws");
        scratch.file("ws/nobs.toml", &workshop_text());
        assert_eq!(locate(&ws, None).unwrap().project(), Some("nobs-t-one"));
        let book = scratch.book_lab("book");
        assert_eq!(locate(&book, None).unwrap().project(), Some("netobs"));
        let lab = scratch.dir("lab");
        scratch.file("lab/docker-compose.yml", "");
        assert_eq!(locate(&lab, None).unwrap().project(), None);
    }

    #[test]
    fn a_settings_file_next_to_the_book_layout_makes_a_book_lab_with_overrides() {
        let scratch = Scratch::new("book-overrides");
        let book = scratch.book_lab("book");
        scratch.file(
            "book/nobs.toml",
            "kind = \"lab\"\ntopology = \"t/x.yml\"\nproject = \"p\"\nnetwork = \"n\"\n",
        );
        let located = locate(&book, None).unwrap();
        assert!(
            matches!(located.kind, Kind::BookLab(Some(_))),
            "{located:?}"
        );
        assert_eq!(located.project(), Some("p"));
        assert_eq!(located.network(), Some("n"));
        assert_eq!(located.topology(), book.join("t/x.yml"));
        assert_eq!(
            located.compose(Some("ch1")),
            book.join("chapters/ch1/docker-compose.yml")
        );
    }

    #[test]
    fn a_book_lab_without_a_settings_file_is_named_netobs_on_network_observability() {
        let scratch = Scratch::new("book-defaults");
        let book = scratch.book_lab("book");
        let located = locate(&book, None).unwrap();
        assert_eq!(located.project(), Some("netobs"));
        assert_eq!(located.network(), Some("network-observability"));
    }

    #[test]
    fn a_labcli_lab_has_a_network_only_when_its_settings_name_one() {
        let scratch = Scratch::new("network");
        let lab = scratch.dir("lab");
        scratch.file("lab/docker-compose.yml", "");
        assert_eq!(locate(&lab, None).unwrap().network(), None);
        scratch.file("lab/nobs.toml", "kind = \"lab\"\n");
        assert_eq!(locate(&lab, None).unwrap().network(), None);
        scratch.file("lab/nobs.toml", "kind = \"lab\"\nnetwork = \"n\"\n");
        assert_eq!(locate(&lab, None).unwrap().network(), Some("n"));
    }

    #[test]
    fn a_bad_nobs_toml_is_an_error_naming_its_path() {
        let scratch = Scratch::new("bad");
        scratch.file("ws/docker-compose.yml", "");
        let file = scratch.file("ws/nobs.toml", "kind = \"lab\"\ncolour = \"red\"\n");
        let expected = LocateError::Settings {
            path: file.clone(),
            reason: "colour: unknown key".to_string(),
        };
        let error = locate(&scratch.dir("ws/a"), None).unwrap_err();
        assert_eq!(error, expected);
        assert_eq!(
            error.to_string(),
            format!("{}: colour: unknown key", file.display())
        );
        assert_eq!(
            locate(&scratch.0, Some(&scratch.0.join("ws"))),
            Err(expected)
        );
    }

    #[test]
    fn find_workshops_from_the_workshops_repo_holds_nobs_t_two_once() {
        assert_eq!(
            find_workshops(&fixture("workshops-repo")),
            [(
                "nobs-t-two".to_string(),
                "Workshop two".to_string(),
                fixture("workshops-repo/workshops/nobs-t-two")
            )]
        );
    }

    #[test]
    fn find_workshops_from_inside_workshop_one_holds_the_located_workshop() {
        assert_eq!(
            find_workshops(&fixture("workshop-one/sub")),
            [(
                "nobs-t-one".to_string(),
                "Workshop one".to_string(),
                fixture("workshop-one")
            )]
        );
    }

    #[test]
    fn find_workshops_from_a_bare_folder_is_empty() {
        let scratch = Scratch::new("find-bare");
        assert_eq!(find_workshops(&scratch.0), []);
    }

    #[test]
    fn the_fixtures_hold_their_kinds_and_names() {
        let text = |relative: &str| fs::read_to_string(fixture(relative)).unwrap();
        let one = text("workshop-one/nobs.toml");
        assert!(one.contains("kind = \"workshop\"\n"), "{one}");
        assert!(one.contains("name = \"nobs-t-one\"\n"), "{one}");
        assert!(one.contains("project = \"nobs-t-one\"\n"), "{one}");
        let two = text("workshops-repo/workshops/nobs-t-two/nobs.toml");
        assert!(two.contains("kind = \"workshop\"\n"), "{two}");
        assert!(two.contains("name = \"nobs-t-two\"\n"), "{two}");
        assert!(two.contains("project = \"nobs-t-two\"\n"), "{two}");
        let lab = text("labcli-yaml/nobs.toml");
        assert!(lab.contains("kind = \"lab\"\n"), "{lab}");
        assert!(lab.contains("project = \"nobs-t-yaml\"\n"), "{lab}");
        for (topology, name) in [
            ("book-lab/containerlab/lab.yml", "nobs-t-book"),
            ("labcli-example/containerlab/lab.clab.yml", "nobs-t-labcli"),
            ("labcli-yaml/containerlab/lab.clab.yaml", "nobs-t-yaml"),
        ] {
            assert!(
                text(topology).starts_with(&format!("name: {name}\n")),
                "{topology}"
            );
        }
    }

    #[test]
    fn find_workshops_skips_a_bad_nobs_toml_and_a_lab_file() {
        let scratch = Scratch::new("find-bad");
        scratch.file("bad/nobs.toml", "kind = \"workshop\"\n");
        scratch.file("lab/nobs.toml", "kind = \"lab\"\n");
        scratch.file("workshops/good/nobs.toml", &workshop_text());
        assert_eq!(
            find_workshops(&scratch.0),
            [(
                "nobs-t-one".to_string(),
                "Workshop one".to_string(),
                scratch.0.join("workshops/good")
            )]
        );
    }
}
