#[path = "support/listing.rs"]
mod listing;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use listing::listing;

const NODES: [&str; 2] = ["nobs-t-srl1", "nobs-t-srl2"];

const LINUX_TOPOLOGY: &str = "name: nobs-t-render
topology:
  nodes:
    nobs-t-host1:
      kind: linux
    nobs-t-host2:
      kind: linux
    nobs-t-host9:
      kind: linux
";

const LINUX_VARS: &str = "nodes:
  nobs-t-host1:
    mgmt-ipv4: \"203.0.113.21\"
  nobs-t-host2:
    mgmt-ipv4: \"203.0.113.22\"
";

struct Lab(PathBuf);

impl Lab {
    fn new(name: &str) -> Lab {
        let root =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-device-render-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Lab(root.canonicalize().unwrap())
    }

    fn project(name: &str) -> Lab {
        let lab = Lab::new(name);
        let fixture = fixture("project-two-node");
        fs::copy(
            fixture.join("nobs_project.yml"),
            lab.0.join("nobs_project.yml"),
        )
        .unwrap();
        lab.write(
            "templates/srl.j2",
            &fs::read_to_string(fixture.join("templates/srl.j2")).unwrap(),
        );
        lab
    }

    fn linux(name: &str) -> Lab {
        let lab = Lab::new(name);
        lab.write("nobs.toml", "kind = \"lab\"\n");
        lab.write("containerlab/lab.clab.yml", LINUX_TOPOLOGY);
        lab.write("containerlab/lab_vars.yml", LINUX_VARS);
        lab.write(
            "templates/linux.j2",
            "{{ name }} {{ kind }} {{ mgmt_ipv4 }}\n",
        );
        lab
    }

    fn write(&self, relative: &str, text: &str) {
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

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(relative)
}

fn nobs(here: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nobs"))
        .args(args)
        .current_dir(here)
        .env_remove("NOBS_DIR")
        .output()
        .unwrap()
}

fn render(dir: &Path, name: &str) -> Output {
    nobs(
        dir,
        &["--dir", dir.to_str().unwrap(), "device", "render", name],
    )
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_fails_with(output: &Output, line: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", stderr(output));
    assert_eq!(stderr(output), format!("{line}\n"));
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn each_node_prints_the_startup_config_project_render_wrote_and_the_folder_stays_the_same() {
    let lab = Lab::project("equal");
    let rendered = nobs(
        &lab.0,
        &["--dir", lab.0.to_str().unwrap(), "project", "render"],
    );
    assert_eq!(rendered.status.code(), Some(0), "{}", stderr(&rendered));

    for node in NODES {
        let before = listing(&lab.0);

        let output = render(&lab.0, node);

        assert_eq!(output.status.code(), Some(0), "{node}: {}", stderr(&output));
        assert_eq!(stderr(&output), "", "{node}");
        let written = fs::read(lab.0.join(format!("containerlab/configs/{node}.cfg"))).unwrap();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(written).unwrap(),
            "{node}"
        );
        assert_eq!(listing(&lab.0), before, "{node}");
    }
}

#[test]
fn an_edit_of_lab_vars_after_project_render_reaches_the_next_render() {
    let lab = Lab::project("edit");
    let rendered = nobs(
        &lab.0,
        &["--dir", lab.0.to_str().unwrap(), "project", "render"],
    );
    assert_eq!(rendered.status.code(), Some(0), "{}", stderr(&rendered));
    let vars = lab.0.join("containerlab/lab_vars.yml");
    let text = fs::read_to_string(&vars).unwrap();
    fs::write(&vars, text.replace("192.0.2.0/31", "198.51.100.0/31")).unwrap();
    let written = fs::read_to_string(lab.0.join("containerlab/configs/nobs-t-srl1.cfg")).unwrap();

    let output = render(&lab.0, "nobs-t-srl1");

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        written.replace("192.0.2.0/31", "198.51.100.0/31")
    );
}

#[test]
fn the_name_the_kind_from_the_topology_and_the_labs_source_reach_the_template() {
    let lab = Lab::linux("hand-over");
    let before = listing(&lab.0);

    for (node, expected) in [
        ("nobs-t-host1", "nobs-t-host1 linux 203.0.113.21\n"),
        ("nobs-t-host2", "nobs-t-host2 linux 203.0.113.22\n"),
    ] {
        let output = render(&lab.0, node);

        assert_eq!(output.status.code(), Some(0), "{node}: {}", stderr(&output));
        assert_eq!(stderr(&output), "", "{node}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected, "{node}");
    }
    assert_eq!(listing(&lab.0), before);
}

#[test]
fn a_name_the_topology_does_not_hold_names_the_topology() {
    let lab = Lab::linux("not-a-node");
    assert_fails_with(
        &render(&lab.0, "nobs-t-srl9"),
        &format!(
            "nobs: render nobs-t-srl9: not a node of {}",
            lab.0.join("containerlab/lab.clab.yml").display()
        ),
    );
}

#[test]
fn in_the_book_lab_the_kind_has_no_template() {
    assert_fails_with(
        &render(&fixture("book-lab"), "nobs-t-srl1"),
        "nobs: render nobs-t-srl1: no template for kind nokia_srlinux: expected templates/srl.j2",
    );
}

#[test]
fn a_lab_without_lab_vars_has_no_source_of_truth() {
    let lab = Lab::linux("no-source");
    fs::remove_file(lab.0.join("containerlab/lab_vars.yml")).unwrap();
    assert_fails_with(
        &render(&lab.0, "nobs-t-host1"),
        "nobs: render nobs-t-host1: this lab has no source of truth",
    );
}

#[test]
fn a_lab_vars_that_does_not_read_gives_the_line_of_the_source() {
    let lab = Lab::linux("bad-source");
    let path = lab.0.join("containerlab/lab_vars.yml");
    fs::write(&path, "nodes: [\n").unwrap();

    let output = render(&lab.0, "nobs-t-host1");

    let stderr = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(output.stdout.is_empty());
    let start = format!("nobs: render nobs-t-host1: {}: ", path.display());
    assert!(stderr.starts_with(&start), "{stderr}");
}

#[test]
fn without_a_template_the_template_line_comes_before_the_source_line() {
    let lab = Lab::linux("no-template");
    fs::remove_file(lab.0.join("templates/linux.j2")).unwrap();
    let line = "nobs: render nobs-t-host1: no template for kind linux: expected templates/linux.j2";
    assert_fails_with(&render(&lab.0, "nobs-t-host1"), line);

    fs::write(lab.0.join("containerlab/lab_vars.yml"), "nodes: [\n").unwrap();
    assert_fails_with(&render(&lab.0, "nobs-t-host1"), line);

    fs::remove_file(lab.0.join("containerlab/lab_vars.yml")).unwrap();
    assert_fails_with(&render(&lab.0, "nobs-t-host1"), line);
}

#[test]
fn a_node_the_source_does_not_know_has_no_intended_configuration() {
    let lab = Lab::linux("unknown");
    assert_fails_with(
        &render(&lab.0, "nobs-t-host9"),
        "nobs: render nobs-t-host9: device nobs-t-host9 not found in the source of truth.",
    );
}

#[test]
fn a_name_the_data_lacks_names_the_device_the_template_and_the_name() {
    let lab = Lab::project("missing-name");
    let rendered = nobs(
        &lab.0,
        &["--dir", lab.0.to_str().unwrap(), "project", "render"],
    );
    assert_eq!(rendered.status.code(), Some(0), "{}", stderr(&rendered));
    let template = lab.0.join("templates/srl.j2");
    let mut text = fs::read_to_string(&template).unwrap();
    text.push_str("{{ nobs_t_missing }}\n");
    fs::write(&template, text).unwrap();

    let output = render(&lab.0, "nobs-t-srl1");

    let stderr = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(output.stdout.is_empty());
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.ends_with('\n'), "{stderr}");
    assert!(
        stderr.starts_with("nobs: render nobs-t-srl1: templates/srl.j2: "),
        "{stderr}"
    );
    assert!(stderr.contains("nobs_t_missing"), "{stderr}");
}

#[test]
fn a_project_not_yet_rendered_gives_the_topology_line() {
    let lab = Lab::project("not-rendered");
    assert_fails_with(
        &render(&lab.0, "nobs-t-srl1"),
        &format!(
            "nobs: topology file not found: {}",
            lab.0.join("containerlab/lab.clab.yml").display()
        ),
    );
}

#[test]
fn in_a_workshop_or_outside_a_lab_the_lines_of_today() {
    let workshop = fixture("workshop-one");
    assert_fails_with(
        &render(&workshop, "nobs-t-srl1"),
        &format!(
            "nobs: device commands need a lab with containerlab nodes; {} is a workshop",
            workshop.display()
        ),
    );

    let bare = Lab::new("no-lab");
    assert_fails_with(
        &nobs(&bare.0, &["device", "render", "nobs-t-srl1"]),
        "nobs: no lab or workshop found from here",
    );
}
