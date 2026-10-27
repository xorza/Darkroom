//! The name a new node takes: its own, or the next free number on it.

use scenarium::Graph as CoreGraph;

/// `wanted` when no node of `graph` has that name, else the base of `wanted` — its name with a
/// trailing ` <number>` taken off — with the smallest number from 2 that no node has: a second
/// "Blend" is "Blend 2", and a copy of "Blend 2" beside it is "Blend 3". Blender names a copy
/// the same way (`Cube`, `Cube.001`); a space and a plain count read as the editor's other text.
pub(super) fn free(graph: &CoreGraph, wanted: &str) -> String {
    let taken = |name: &str| graph.iter().any(|node| node.name == name);
    if !taken(wanted) {
        return wanted.to_owned();
    }
    let base = match wanted.rsplit_once(' ') {
        Some((base, number))
            if !base.is_empty() && number.parse::<usize>().is_ok_and(|n| n >= 2) =>
        {
            base
        }
        _ => wanted,
    };
    // The graph's nodes take at most one number each, so one of the node count plus one numbers
    // from 2 is free.
    let node_count = graph.iter().count();
    (2..=node_count + 2)
        .map(|n| format!("{base} {n}"))
        .find(|name| !taken(name))
        .expect("n nodes leave one of n + 1 numbers free")
}

#[cfg(test)]
mod tests;
