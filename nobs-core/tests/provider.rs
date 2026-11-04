use nobs_core::compose::Compose;
use nobs_core::containerlab::Containerlab;
use nobs_core::provider::Provider;

const FILE: &str = "lab/lab.yaml";

#[test]
fn compose_lists_the_lab_with_docker_compose_ps_all() {
    let compose = Compose::new(FILE);
    let provider: &dyn Provider = &compose;

    assert_eq!(provider.program(), "docker");
    assert_eq!(provider.show_args(), ["compose", "-f", FILE, "ps", "--all"]);
}

#[test]
fn containerlab_without_sudo_lists_the_lab_with_containerlab_inspect() {
    let containerlab = Containerlab::new(FILE);
    let provider: &dyn Provider = &containerlab;

    assert_eq!(provider.program(), "containerlab");
    assert_eq!(provider.show_args(), ["inspect", "-t", FILE]);
}

#[test]
fn containerlab_with_sudo_runs_sudo_and_keeps_the_named_variables() {
    let containerlab = Containerlab::new(FILE)
        .sudo(true)
        .preserve_env(vec!["A".into()]);
    let provider: &dyn Provider = &containerlab;

    assert_eq!(provider.program(), "sudo");
    assert_eq!(
        provider.show_args(),
        ["--preserve-env=A", "containerlab", "inspect", "-t", FILE]
    );
}
