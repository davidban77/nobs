use nobs_core::lab::TopologyNode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRow {
    pub name: String,
    pub kind: String,
    pub state: Option<String>,
}

pub fn container_states(ps_text: &str) -> Vec<(String, String)> {
    ps_text
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(name, state)| (name.to_string(), state.to_string()))
        .collect()
}

pub fn node_rows(topology: &[TopologyNode], states: &[(String, String)]) -> Vec<NodeRow> {
    topology
        .iter()
        .map(|node| NodeRow {
            name: node.name.clone(),
            kind: node.kind.clone(),
            state: states
                .iter()
                .find(|(name, _)| *name == node.container)
                .map(|(_, state)| state.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, container: &str) -> TopologyNode {
        TopologyNode {
            name: name.to_string(),
            kind: "nokia_srlinux".to_string(),
            container: container.to_string(),
        }
    }

    fn pair(name: &str, state: &str) -> (String, String) {
        (name.to_string(), state.to_string())
    }

    #[test]
    fn container_states_reads_one_name_and_state_per_line_and_skips_a_line_without_a_tab() {
        let text = "clab-lab-srl1\trunning\nno tab here\nclab-lab-srl2\texited\n";

        assert_eq!(
            container_states(text),
            [
                pair("clab-lab-srl1", "running"),
                pair("clab-lab-srl2", "exited")
            ]
        );
    }

    #[test]
    fn a_node_takes_its_container_state_a_stranger_gives_no_row_and_an_unlisted_node_has_none() {
        let topology = [node("srl1", "clab-lab-srl1"), node("srl2", "clab-lab-srl2")];
        let states = container_states("clab-lab-srl1\texited\ngrafana\trunning\n");

        let rows = node_rows(&topology, &states);

        assert_eq!(
            rows,
            [
                NodeRow {
                    name: "srl1".to_string(),
                    kind: "nokia_srlinux".to_string(),
                    state: Some("exited".to_string()),
                },
                NodeRow {
                    name: "srl2".to_string(),
                    kind: "nokia_srlinux".to_string(),
                    state: None,
                },
            ]
        );
    }
}
