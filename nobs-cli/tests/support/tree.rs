pub fn commands<'a>(command: &'a clap::Command, path: &str) -> Vec<(String, &'a clap::Command)> {
    let mut found = vec![(path.to_string(), command)];
    for sub in command.get_subcommands() {
        found.extend(commands(sub, &format!("{path} {}", sub.get_name())));
    }
    found
}
