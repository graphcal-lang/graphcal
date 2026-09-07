use crate::config::Baseline;
use crate::model::{Edge, EdgeKey};
use std::collections::BTreeSet;

#[derive(Debug, Eq, PartialEq)]
pub struct RatchetReport {
    pub stale: BTreeSet<EdgeKey>,
    pub new: BTreeSet<EdgeKey>,
}

pub fn compare(current: &[Edge], baseline: &Baseline) -> RatchetReport {
    let current_keys = current
        .iter()
        .map(|edge| edge.key.clone())
        .collect::<BTreeSet<_>>();
    let baseline_keys = baseline.exceptions.keys().cloned().collect::<BTreeSet<_>>();
    RatchetReport {
        stale: baseline_keys.difference(&current_keys).cloned().collect(),
        new: current_keys.difference(&baseline_keys).cloned().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModuleId, Package};
    use std::collections::BTreeMap;

    fn key(test_only: bool) -> EdgeKey {
        EdgeKey {
            from: ModuleId::new(Package::Eval, vec!["from".into()]),
            to: ModuleId::new(Package::Eval, vec!["to".into()]),
            test_only,
        }
    }
    fn edge(key: EdgeKey) -> Edge {
        Edge {
            key,
            evidence: Vec::new(),
        }
    }

    #[test]
    fn production_and_test_edges_are_distinct_ratchet_keys() {
        let production = key(false);
        let test_only = key(true);
        let baseline = Baseline {
            exceptions: BTreeMap::from([(production.clone(), "production reason".into())]),
        };
        let result = compare(&[edge(production), edge(test_only.clone())], &baseline);
        assert!(result.stale.is_empty());
        assert_eq!(result.new, BTreeSet::from([test_only]));
    }
}
