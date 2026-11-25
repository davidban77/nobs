use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use nobs_core::check::{self, Reading, Verdict};
use nobs_core::device::{self, Error, FakeTransport};
use nobs_core::intended::Failure;
use nobs_core::lab::TopologyNode;
use nobs_core::locate::locate;
use nobs_core::runner::Output;
use nobs_core::status::Urls;
use nobs_tui::shell::{stacks, Agreement};
use nobs_tui::{Shell, SondaTarget, Start, Update, Workbench};

const INTENDED: &str = "set / a b on\nset / c d on\n";
const SYNCED: &str = "set / a b on\nset / c d on\nset / e f on\n";
const LACKING: &str = "set / a b off\n";
const OLD: &str = "set / a b on\n";

fn model() -> Shell {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/device-lab");
    let lab = locate(&dir, Some(&dir)).unwrap();
    let stacks = stacks(Some(&lab), Vec::new()).unwrap();
    Shell::new(Start {
        nodes: lab.nodes(),
        topology: Some(lab.topology()),
        lab: Some(lab),
        stacks,
        sonda: SondaTarget {
            server: None,
            api_key: None,
        },
        home: Workbench::Devices,
        chooser: false,
        source: None,
        source_error: None,
        source_service: None,
        urls: Urls::defaults(),
        program: PathBuf::from("nobs"),
        login: Arc::new(|_| None),
    })
}

fn node(name: &str, kind: &str) -> TopologyNode {
    TopologyNode {
        name: name.to_string(),
        kind: kind.to_string(),
        container: format!("clab-{name}"),
    }
}

fn replied(stdout: &str) -> Result<Output, Error> {
    Ok(Output {
        code: 0,
        stdout: stdout.to_string(),
        stderr: String::new(),
    })
}

fn intended_of(node: &TopologyNode) -> Result<String, Failure> {
    match node.name.as_str() {
        "nobs-t-linux1" => Err(Failure::NoTemplate("no template for kind linux".into())),
        "nobs-t-srl4" => Err(Failure::Other("nobs-t-source-error".into())),
        _ => Ok(INTENDED.to_string()),
    }
}

fn cell_of(words: &str, before: Agreement) -> Agreement {
    if words == "in sync" {
        return Agreement::InSync;
    }
    if let Some(count) = words.strip_suffix(" missing") {
        return Agreement::Missing(count.parse().unwrap());
    }
    if words.starts_with("failed: ") {
        return Agreement::Failed;
    }
    if words.starts_with("not checked: ") || words.starts_with("no intended text: ") {
        return Agreement::Unknown;
    }
    assert_eq!(words, "not tried");
    before
}

fn kind(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Answered { .. } => "answered",
        Verdict::Failed(_) => "failed",
        Verdict::NotTried => "not tried",
        Verdict::NotChecked(_) => "not checked",
        Verdict::NoIntendedText(_) => "no intended text",
    }
}

#[test]
fn the_check_line_and_the_cell_say_the_same_after_a_walk_saved_and_loaded() {
    std::env::set_var(device::USER, "admin");
    std::env::set_var(device::PASSWORD, "nobs-t-not-a-secret");
    let nodes = [
        node("nobs-t-srl1", "nokia_srlinux"),
        node("nobs-t-srl2", "srl"),
        node("nobs-t-srl3", "nokia_srlinux"),
        node("nobs-t-linux1", "linux"),
        node("nobs-t-srl4", "srl"),
        node("nobs-t-srl5", "srl"),
        node("nobs-t-srl6", "nokia_srlinux"),
    ];
    let transport = FakeTransport::per_host(vec![
        ("clab-nobs-t-srl1".to_string(), replied(SYNCED)),
        ("clab-nobs-t-srl2".to_string(), replied(LACKING)),
        (
            "clab-nobs-t-srl3".to_string(),
            Err(Error::Unreachable {
                host: "clab-nobs-t-srl3".to_string(),
                reason: "connection refused".to_string(),
            }),
        ),
        (
            "clab-nobs-t-srl5".to_string(),
            Err(Error::AuthenticationFailed),
        ),
        ("clab-nobs-t-srl6".to_string(), replied(SYNCED)),
    ]);
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("check-cells");
    let _ = fs::remove_dir_all(&folder);

    let readings: Vec<Reading> = check::walk(&nodes, &transport, &intended_of, &mut |_| {});
    check::make_folder(&folder).unwrap();
    for reading in &readings {
        check::save(&folder, reading).unwrap();
    }
    let loaded = check::load(&folder);
    fs::remove_dir_all(&folder).unwrap();

    let mut model = model();
    model.apply(Update::Intended(
        nodes
            .iter()
            .map(|node| {
                (
                    node.name.clone(),
                    intended_of(node),
                    Some(device::Syntax::SrLinux),
                )
            })
            .collect(),
    ));
    model.apply(Update::Checked(vec![(
        "nobs-t-srl6".to_string(),
        Ok(OLD.lines().map(str::to_string).collect()),
    )]));
    let before: Vec<Agreement> = nodes
        .iter()
        .map(|node| model.agreement(&node.name))
        .collect();
    model.apply(Update::Checked(loaded));

    for (reading, before) in readings.iter().zip(before) {
        let said = check::line(0, reading);
        let words = said.strip_prefix(&format!("{}  ", reading.name)).unwrap();
        assert_eq!(
            model.agreement(&reading.name),
            cell_of(words, before),
            "{said}"
        );
        if let Some(error) = words.strip_prefix("failed: ") {
            assert_eq!(model.fetch_error(&reading.name), Some(error), "{said}");
        }
    }
    assert_eq!(
        readings
            .iter()
            .map(|reading| kind(&reading.verdict))
            .collect::<Vec<_>>(),
        [
            "answered",
            "answered",
            "failed",
            "not checked",
            "no intended text",
            "failed",
            "not tried"
        ]
    );
    assert_eq!(model.agreement("nobs-t-srl6"), Agreement::Missing(1));
}
