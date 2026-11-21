//! The depth-first walks a graph's and a set's rules share: [`back_edges`] for an order and the
//! edges that close a loop, [`reaches`] for whether one node leads to another. Iterative, so a
//! deep relation costs heap, not stack, and each node is entered once, so a loop ends the walk
//! rather than the process. As petgraph's `depth_first_search`, whose `DfsEvent::BackEdge` is
//! "an edge to an already visited node"
//! ([`DfsEvent`](https://docs.rs/petgraph/latest/petgraph/visit/enum.DfsEvent.html)); refused,
//! petgraph itself, a dependency for two functions over ids the graphs already key by.

use std::hash::Hash;

use hashbrown::{HashMap, HashSet};

/// What a walk found: each edge that returns to a node on the current path, in the order met,
/// and every node reached, each after every node it leads to that is not on the path.
#[derive(Debug)]
pub(crate) struct BackEdges<N, E> {
    pub(crate) closing: Vec<E>,
    pub(crate) post_order: Vec<N>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    OnPath,
    Done,
}

#[derive(Debug)]
enum Visit<N, E> {
    Enter(N),
    Edge(E, N),
    Exit(N),
}

/// Walk from each of `roots` in turn, following the edges `successors` pushes for a node, in the
/// order it pushes them. An edge to a node on the current path closes a loop and is not
/// followed; removing every closing edge leaves the relation acyclic, and the same roots and
/// order make the same choice on every walk.
pub(crate) fn back_edges<N, E>(
    roots: impl IntoIterator<Item = N>,
    mut successors: impl FnMut(N, &mut Vec<(E, N)>),
) -> BackEdges<N, E>
where
    N: Copy + Eq + Hash,
    E: Copy,
{
    let mut marks: HashMap<N, Mark> = HashMap::new();
    let mut stack: Vec<Visit<N, E>> = Vec::new();
    let mut next: Vec<(E, N)> = Vec::new();
    let mut walk = BackEdges {
        closing: Vec::new(),
        post_order: Vec::new(),
    };
    for root in roots {
        if marks.contains_key(&root) {
            continue;
        }
        stack.push(Visit::Enter(root));
        while let Some(visit) = stack.pop() {
            match visit {
                Visit::Enter(node) => {
                    marks.insert(node, Mark::OnPath);
                    stack.push(Visit::Exit(node));
                    successors(node, &mut next);
                    // Reversed onto the stack, so they are taken in the order pushed.
                    stack.extend(next.iter().rev().map(|&(edge, to)| Visit::Edge(edge, to)));
                    next.clear();
                }
                Visit::Edge(edge, to) => match marks.get(&to) {
                    Some(Mark::OnPath) => walk.closing.push(edge),
                    Some(Mark::Done) => {}
                    None => stack.push(Visit::Enter(to)),
                },
                Visit::Exit(node) => {
                    marks.insert(node, Mark::Done);
                    walk.post_order.push(node);
                }
            }
        }
    }
    walk
}

/// Whether `to` is `from` or lies on a path from it through `successors`.
pub(crate) fn reaches<N>(from: N, to: N, mut successors: impl FnMut(N, &mut Vec<N>)) -> bool
where
    N: Copy + Eq + Hash,
{
    let mut seen: HashSet<N> = HashSet::from_iter([from]);
    let mut stack = vec![from];
    let mut next = Vec::new();
    while let Some(node) = stack.pop() {
        if node == to {
            return true;
        }
        successors(node, &mut next);
        stack.extend(next.iter().copied().filter(|&node| seen.insert(node)));
        next.clear();
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::graph::dfs::{back_edges, reaches};

    /// `1 → 2 → 3 → 1` and `1 → 4`, from 1 then 4: the edge `3 → 1` returns to the path and
    /// closes the loop, every node finishes after what it leads to, and 4, already done, is no
    /// root again.
    #[test]
    fn a_walk_names_the_edge_that_closes_a_loop() {
        let edges = [(1, 2), (2, 3), (3, 1), (1, 4)];
        let successors = |node: u32, out: &mut Vec<((u32, u32), u32)>| {
            out.extend(
                edges
                    .iter()
                    .filter(|(from, _)| *from == node)
                    .map(|&(from, to)| ((from, to), to)),
            );
        };
        let walk = back_edges([1, 4], successors);
        assert_eq!(walk.closing, [(3, 1)]);
        assert_eq!(walk.post_order, [3, 2, 4, 1]);

        let walk = back_edges([2], successors);
        assert_eq!(walk.closing, [(1, 2)], "the root decides which edge closes");
        assert_eq!(walk.post_order, [4, 1, 3, 2]);
    }

    /// A node reaches itself, what lies downstream, and nothing else, loops included.
    #[test]
    fn reaches_follows_paths_through_loops() {
        let edges = [(1, 2), (2, 1), (2, 3), (4, 1)];
        let successors = |node: u32, out: &mut Vec<u32>| {
            out.extend(
                edges
                    .iter()
                    .filter(|(from, _)| *from == node)
                    .map(|&(_, to)| to),
            );
        };
        assert!(reaches(1, 1, successors));
        assert!(reaches(1, 3, successors));
        assert!(!reaches(1, 4, successors));
        assert!(!reaches(3, 1, successors));
    }
}
