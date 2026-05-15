use std::collections::VecDeque;

/// Kahn's topological sort on nodes `0..dependents.len()`.
/// `dependents[i]` lists the nodes that depend on node `i` (i.e. the edges i → j).
/// Returns `(order, had_cycle)`. When `had_cycle` is true, `order.len() < n` and
/// cycle members are omitted — callers handle them however they like.
pub fn topological_sort(dependents: &[Vec<usize>]) -> (Vec<usize>, bool) {
    let n = dependents.len();
    let mut in_deg = vec![0usize; n];
    for deps in dependents {
        for &j in deps {
            in_deg[j] += 1;
        }
    }
    let mut queue: VecDeque<usize> = in_deg
        .iter()
        .enumerate()
        .filter_map(|(i, deg)| if *deg == 0 { Some(i) } else { None })
        .collect();
    let mut order = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i);
        for &j in &dependents[i] {
            in_deg[j] -= 1;
            if in_deg[j] == 0 {
                queue.push_back(j);
            }
        }
    }
    let had_cycle = order.len() != n;
    (order, had_cycle)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        fn multi_hop_chain() {
            // 0 → 1 → 2 → 3
            let (order, had_cycle) = topological_sort(&[vec![1], vec![2], vec![3], vec![]]);
            assert_eq!(vec![0, 1, 2, 3], order);
            assert!(!had_cycle);
        }
    }
}
