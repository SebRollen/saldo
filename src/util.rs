use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Kahn's topological sort on nodes `0..dependents.len()`, taking the lowest
/// ready node first, so nodes that don't depend on each other keep their
/// order. `dependents[i]` lists the nodes that depend on node `i` (i.e. the
/// edges i → j). Returns `(order, had_cycle)`. When `had_cycle` is true,
/// `order.len() < n` and cycle members are omitted — callers handle them
/// however they like.
pub fn topological_sort(dependents: &[Vec<usize>]) -> (Vec<usize>, bool) {
    let n = dependents.len();
    let mut in_deg = vec![0usize; n];
    for deps in dependents {
        for &j in deps {
            in_deg[j] += 1;
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> = in_deg
        .iter()
        .enumerate()
        .filter_map(|(i, deg)| (*deg == 0).then_some(Reverse(i)))
        .collect();
    let mut order = Vec::with_capacity(n);
    while let Some(Reverse(i)) = ready.pop() {
        order.push(i);
        for &j in &dependents[i] {
            in_deg[j] -= 1;
            if in_deg[j] == 0 {
                ready.push(Reverse(j));
            }
        }
    }
    let had_cycle = order.len() != n;
    (order, had_cycle)
}

/// How many single-character insertions, deletions and substitutions turn
/// `a` into `b`.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != *cb);
            current.push(substitute.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

/// The candidate closest to `name`, if one is close enough to be a typo.
pub fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (name.chars().count() / 3).clamp(1, 3);
    candidates
        .into_iter()
        .map(|c| (edit_distance(name, c), c))
        .filter(|(distance, _)| *distance <= limit)
        .min()
        .map(|(_, c)| c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distances() {
        assert_eq!(edit_distance("cash", "cash"), 0);
        assert_eq!(edit_distance("csh", "cash"), 1);
        assert_eq!(edit_distance("retirment", "retirement"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn closest_finds_likely_typos() {
        let names = ["assets.cash", "assets.retirement", "expenses.rent"];
        assert_eq!(closest("assets.csh", names), Some("assets.cash"));
        assert_eq!(closest("expenses.food", names), None);
    }

    mod topological_sort {
        use super::*;

        #[test]
        fn empty() {
            let (order, had_cycle) = topological_sort(&[]);
            assert!(order.is_empty());
            assert!(!had_cycle);
        }

        #[test]
        fn no_dependencies() {
            let (order, had_cycle) = topological_sort(&[Vec::new(), Vec::new(), Vec::new()]);
            assert_eq!(vec![0, 1, 2], order);
            assert!(!had_cycle);
        }

        #[test]
        fn self_reference_is_cycle() {
            let (order, had_cycle) = topological_sort(&[vec![0]]);
            assert!(order.is_empty());
            assert!(had_cycle);
        }

        #[test]
        fn orders_according_to_deps() {
            let (order, had_cycle) = topological_sort(&[vec![], vec![0]]);
            assert_eq!(vec![1, 0], order);
            assert!(!had_cycle);
        }

        #[test]
        fn keeps_non_cyclical_deps() {
            let (order, had_cycle) = topological_sort(&[vec![1], vec![0], vec![], vec![2]]);
            assert_eq!(vec![3, 2], order);
            assert!(had_cycle);
        }

        #[test]
        fn all_cycle() {
            let (order, had_cycle) = topological_sort(&[vec![1], vec![2], vec![0]]);
            assert!(order.is_empty());
            assert!(had_cycle);
        }

        #[test]
        fn diamond() {
            // 0 → 1, 0 → 2, 1 → 3, 2 → 3
            let (order, had_cycle) = topological_sort(&[vec![1, 2], vec![3], vec![3], vec![]]);
            assert!(!had_cycle);
            assert_eq!(order[0], 0);
            assert_eq!(order[3], 3);
            assert!(order[1] == 1 || order[1] == 2);
        }

        #[test]
        fn independent_nodes_keep_their_order() {
            // 2 must come before 0; 1 and 3 depend on nothing.
            let (order, had_cycle) = topological_sort(&[vec![], vec![], vec![0], vec![]]);
            assert_eq!(vec![1, 2, 0, 3], order);
            assert!(!had_cycle);
        }

        #[test]
        fn multi_hop_chain() {
            // 0 → 1 → 2 → 3
            let (order, had_cycle) = topological_sort(&[vec![1], vec![2], vec![3], vec![]]);
            assert_eq!(vec![0, 1, 2, 3], order);
            assert!(!had_cycle);
        }
    }
}
