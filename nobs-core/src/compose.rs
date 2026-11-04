use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub struct Compose {
    file: PathBuf,
    profiles: Vec<String>,
    verbose: bool,
}

impl Compose {
    pub const PROGRAM: &'static str = "docker";

    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            profiles: Vec::new(),
            verbose: false,
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    pub fn with_profile(mut self, profile: &str) -> Self {
        self.profiles.push(profile.to_string());
        self
    }

    pub fn verbose(mut self, on: bool) -> Self {
        self.verbose = on;
        self
    }

    pub fn ps_args(&self, services: &[String]) -> Vec<OsString> {
        self.compose_args(words(&["ps", "--all"]), services)
    }

    pub fn build_args(&self, services: &[String]) -> Vec<OsString> {
        self.compose_args(words(&["build"]), services)
    }

    pub fn pull_args(&self, services: &[String]) -> Vec<OsString> {
        self.compose_args(words(&["pull"]), services)
    }

    pub fn debug_args(&self, services: &[String]) -> Vec<OsString> {
        self.compose_args(words(&["up", "--remove-orphans"]), services)
    }

    pub fn start_args(&self, services: &[String]) -> Vec<OsString> {
        self.compose_args(words(&["up", "-d", "--remove-orphans"]), services)
    }

    pub fn stop_args(&self, services: &[String]) -> Vec<OsString> {
        self.compose_args(words(&["stop"]), services)
    }

    pub fn restart_args(&self, services: &[String]) -> Vec<OsString> {
        self.compose_args(words(&["restart"]), services)
    }

    pub fn logs_args(&self, services: &[String], follow: bool, tail: Option<u32>) -> Vec<OsString> {
        let mut action = words(&["logs"]);
        if follow {
            action.push("-f".into());
        }
        if let Some(lines) = tail {
            action.push(format!("--tail={lines}").into());
        }
        self.compose_args(action, services)
    }

    pub fn destroy_args(&self, services: &[String], volumes: bool) -> Vec<OsString> {
        let mut action = words(&["down"]);
        if volumes {
            action.push("--volumes".into());
        }
        action.push("--remove-orphans".into());
        self.compose_args(action, services)
    }

    pub fn rm_args(&self, services: &[String], volumes: bool, force: bool) -> Vec<OsString> {
        let mut action = words(&["rm", "--stop"]);
        if force {
            action.push("--force".into());
        }
        if volumes {
            action.push("--volumes".into());
        }
        self.compose_args(action, services)
    }

    pub fn exec_args(&self, service: &str, command: &[OsString], tty: bool) -> Vec<OsString> {
        let mut action = words(&["exec"]);
        if !tty {
            action.push("-T".into());
        }
        action.push(service.into());
        action.extend(command.iter().cloned());
        self.compose_args(action, &[])
    }

    pub fn update_args(&self, services: &[String], volumes: bool) -> [Vec<OsString>; 2] {
        let mut action = words(&["up", "-d", "--remove-orphans", "--force-recreate"]);
        if volumes {
            action.push("--renew-anon-volumes".into());
        }
        [
            self.pull_args(services),
            self.compose_args(action, services),
        ]
    }

    fn compose_args(&self, action: Vec<OsString>, services: &[String]) -> Vec<OsString> {
        let mut args = vec![
            "compose".into(),
            "-f".into(),
            self.file.clone().into_os_string(),
        ];
        for profile in &self.profiles {
            args.push("--profile".into());
            args.push(profile.into());
        }
        if self.verbose {
            args.push("--verbose".into());
        }
        args.extend(action);
        args.extend(services.iter().map(OsString::from));
        args
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkAction {
    Connect,
    Create,
    Disconnect,
    Inspect,
    Ls,
    Prune,
    Rm,
}

impl NetworkAction {
    fn word(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Create => "create",
            Self::Disconnect => "disconnect",
            Self::Inspect => "inspect",
            Self::Ls => "ls",
            Self::Prune => "prune",
            Self::Rm => "rm",
        }
    }
}

pub fn network_args(
    action: NetworkAction,
    name: Option<&str>,
    driver: Option<&str>,
    subnet: Option<&str>,
) -> Vec<OsString> {
    let mut args = words(&["network", action.word()]);
    if action == NetworkAction::Create {
        if let Some(driver) = driver {
            args.push(format!("--driver={driver}").into());
        }
        if let Some(subnet) = subnet {
            args.push(format!("--subnet={subnet}").into());
        }
    }
    let takes_name = !matches!(action, NetworkAction::Ls | NetworkAction::Prune);
    if let Some(name) = name.filter(|_| takes_name) {
        args.push(name.into());
    }
    args
}

fn words(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "lab/docker-compose.yml";

    fn services(list: &[&str]) -> Vec<String> {
        list.iter().map(|service| service.to_string()).collect()
    }

    fn compose() -> Compose {
        Compose::new(FILE)
    }

    #[test]
    fn ps_runs_docker_compose_ps_all_on_the_file() {
        let compose = Compose::new("lab/docker-compose.yml");
        assert_eq!(Compose::PROGRAM, "docker");
        assert_eq!(
            compose.ps_args(&[]),
            ["compose", "-f", "lab/docker-compose.yml", "ps", "--all"]
        );
    }

    #[test]
    fn build_lists_the_services_after_build() {
        assert_eq!(
            compose().build_args(&services(&["telegraf-01", "loki"])),
            ["compose", "-f", FILE, "build", "telegraf-01", "loki"]
        );
    }

    #[test]
    fn pull_lists_the_services_after_pull() {
        assert_eq!(
            compose().pull_args(&services(&["telegraf-01"])),
            ["compose", "-f", FILE, "pull", "telegraf-01"]
        );
    }

    #[test]
    fn debug_is_up_remove_orphans_in_the_foreground() {
        assert_eq!(
            compose().debug_args(&services(&["telegraf-01"])),
            [
                "compose",
                "-f",
                FILE,
                "up",
                "--remove-orphans",
                "telegraf-01"
            ]
        );
    }

    #[test]
    fn start_is_up_detached_with_remove_orphans() {
        assert_eq!(
            compose().start_args(&services(&["telegraf-01"])),
            [
                "compose",
                "-f",
                FILE,
                "up",
                "-d",
                "--remove-orphans",
                "telegraf-01"
            ]
        );
    }

    #[test]
    fn stop_lists_the_services_after_stop() {
        assert_eq!(
            compose().stop_args(&services(&["telegraf-01"])),
            ["compose", "-f", FILE, "stop", "telegraf-01"]
        );
    }

    #[test]
    fn restart_lists_the_services_after_restart() {
        assert_eq!(
            compose().restart_args(&services(&["telegraf-01"])),
            ["compose", "-f", FILE, "restart", "telegraf-01"]
        );
    }

    #[test]
    fn logs_with_follow_and_tail_emits_short_f_and_tail_as_one_token() {
        assert_eq!(
            compose().logs_args(&services(&["telegraf-01"]), true, Some(10)),
            [
                "compose",
                "-f",
                FILE,
                "logs",
                "-f",
                "--tail=10",
                "telegraf-01"
            ]
        );
    }

    #[test]
    fn ps_lists_the_services_after_ps_all() {
        assert_eq!(
            compose().ps_args(&services(&["telegraf-01"])),
            ["compose", "-f", FILE, "ps", "--all", "telegraf-01"]
        );
    }

    #[test]
    fn destroy_with_volumes_puts_volumes_before_remove_orphans() {
        assert_eq!(
            compose().destroy_args(&services(&["telegraf-01"]), true),
            [
                "compose",
                "-f",
                FILE,
                "down",
                "--volumes",
                "--remove-orphans",
                "telegraf-01"
            ]
        );
    }

    #[test]
    fn rm_with_force_and_volumes_orders_stop_force_volumes() {
        assert_eq!(
            compose().rm_args(&services(&["telegraf-01"]), true, true),
            [
                "compose",
                "-f",
                FILE,
                "rm",
                "--stop",
                "--force",
                "--volumes",
                "telegraf-01"
            ]
        );
    }

    #[test]
    fn exec_with_a_terminal_puts_the_service_then_the_command() {
        assert_eq!(
            compose().exec_args("telegraf-01", &words(&["ls", "-la"]), true),
            ["compose", "-f", FILE, "exec", "telegraf-01", "ls", "-la"]
        );
    }

    #[test]
    fn update_pulls_then_recreates_the_same_services() {
        let [pull, up] = compose().update_args(&services(&["telegraf-01", "loki"]), false);
        assert_eq!(pull, ["compose", "-f", FILE, "pull", "telegraf-01", "loki"]);
        assert_eq!(
            up,
            [
                "compose",
                "-f",
                FILE,
                "up",
                "-d",
                "--remove-orphans",
                "--force-recreate",
                "telegraf-01",
                "loki"
            ]
        );
    }

    #[test]
    fn network_create_emits_driver_and_subnet_in_the_equals_form_then_the_name() {
        assert_eq!(
            network_args(
                NetworkAction::Create,
                Some("labcli"),
                Some("bridge"),
                Some("198.51.100.0/24")
            ),
            [
                "network",
                "create",
                "--driver=bridge",
                "--subnet=198.51.100.0/24",
                "labcli"
            ]
        );
    }

    #[test]
    fn profiles_and_verbose_come_right_after_the_file_in_the_order_given() {
        let compose = compose().with_profile("a").with_profile("b").verbose(true);
        assert_eq!(
            compose.build_args(&[]),
            [
                "compose",
                "-f",
                FILE,
                "--profile",
                "a",
                "--profile",
                "b",
                "--verbose",
                "build"
            ]
        );
    }

    #[test]
    fn logs_keeps_a_zero_tail_and_emits_nothing_when_neither_is_asked() {
        assert_eq!(
            compose().logs_args(&[], false, Some(0)),
            ["compose", "-f", FILE, "logs", "--tail=0"]
        );
        assert_eq!(
            compose().logs_args(&[], false, None),
            ["compose", "-f", FILE, "logs"]
        );
    }

    #[test]
    fn exec_without_a_terminal_puts_dash_t_right_after_exec() {
        assert_eq!(
            compose().exec_args("telegraf-01", &words(&["bash"]), false),
            ["compose", "-f", FILE, "exec", "-T", "telegraf-01", "bash"]
        );
    }

    #[test]
    fn update_with_volumes_renews_anonymous_volumes_on_the_second_list_only() {
        let renew: OsString = "--renew-anon-volumes".into();
        let [pull, up] = compose().update_args(&services(&["loki"]), true);
        assert!(!pull.contains(&renew), "{pull:?}");
        assert_eq!(
            up,
            [
                "compose",
                "-f",
                FILE,
                "up",
                "-d",
                "--remove-orphans",
                "--force-recreate",
                "--renew-anon-volumes",
                "loki"
            ]
        );
        let [pull, up] = compose().update_args(&services(&["loki"]), false);
        assert!(!pull.contains(&renew), "{pull:?}");
        assert!(!up.contains(&renew), "{up:?}");
    }

    #[test]
    fn network_ls_and_prune_ignore_the_name() {
        assert_eq!(
            network_args(NetworkAction::Ls, Some("x"), None, None),
            ["network", "ls"]
        );
        assert_eq!(
            network_args(NetworkAction::Prune, Some("x"), None, None),
            ["network", "prune"]
        );
    }

    #[test]
    fn network_inspect_ignores_the_driver_and_keeps_the_name() {
        assert_eq!(
            network_args(NetworkAction::Inspect, Some("NAME"), Some("bridge"), None),
            ["network", "inspect", "NAME"]
        );
    }
}
