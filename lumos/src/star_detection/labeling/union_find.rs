//! The disjoint-set structure that resolves provisional run labels into components.

use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::mem;
use std::sync::atomic::{AtomicU32, Ordering};

/// Lock-free union-find over provisional run labels.
///
/// Operations take `&self` because the strips share one instance across threads. [`Self::reset`]
/// must run before each labeling.
#[derive(Default)]
pub(super) struct UnionFind {
    parent: Vec<AtomicU32>,
    next_label: AtomicU32,
}

impl Debug for UnionFind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnionFind")
            .field("len", &self.parent.len())
            .field("next_label", &self.next_label.load(Ordering::Relaxed))
            .finish()
    }
}

impl UnionFind {
    /// Start a labeling of at most `capacity` provisional labels. The parent table only grows; an
    /// entry is written by [`Self::make_set`] before anything reads it, so stale entries from an
    /// earlier labeling are never seen.
    pub(super) fn reset(&mut self, capacity: usize) {
        if self.parent.len() < capacity {
            self.parent.resize_with(capacity, || AtomicU32::new(0));
        }
        *self.next_label.get_mut() = 1;
    }

    #[inline]
    pub(super) fn make_set(&self) -> u32 {
        // SeqCst: labels must be globally unique across threads.
        let label = self.next_label.fetch_add(1, Ordering::SeqCst);
        debug_assert!(
            (label as usize) <= self.parent.len(),
            "UnionFind capacity exceeded: label {label} > capacity {}",
            self.parent.len()
        );
        self.parent[label as usize - 1].store(label, Ordering::SeqCst);
        label
    }

    #[inline]
    pub(super) fn find(&self, label: u32) -> u32 {
        let mut current = label;
        loop {
            // Relaxed: find is idempotent — stale reads just cause extra
            // iterations, union's CAS provides the synchronization.
            let parent = self.parent_of(current).load(Ordering::Relaxed);
            if parent == current {
                return current;
            }
            let grandparent = self.parent_of(parent).load(Ordering::Relaxed);
            // Path halving. The grandparent is an ancestor of `current` whatever other threads did
            // in between, so the shortcut keeps `current` in its component and keeps
            // `parent <= label`; only non-roots are rewritten, so it never races `union`'s CAS on a
            // root. A lost exchange leaves the longer path, which is still correct.
            if grandparent != parent {
                let _ = self.parent_of(current).compare_exchange_weak(
                    parent,
                    grandparent,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
            current = grandparent;
        }
    }

    pub(super) fn union(&self, a: u32, b: u32) {
        let mut root_a = self.find(a);
        let mut root_b = self.find(b);

        while root_a != root_b {
            if root_a > root_b {
                mem::swap(&mut root_a, &mut root_b);
            }

            // AcqRel: acquire sees prior unions, release publishes this union.
            // Relaxed on failure: we re-find roots anyway.
            match self.parent_of(root_b).compare_exchange_weak(
                root_b,
                root_a,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(current) => {
                    root_a = self.find(root_a);
                    root_b = self.find(current);
                }
            }
        }
    }

    /// The parent slot of a label `make_set` handed out. Every label a caller holds came from
    /// `make_set`, which wrote its slot first, so an unwritten (zero) slot is a broken invariant.
    #[inline]
    fn parent_of(&self, label: u32) -> &AtomicU32 {
        let slot = &self.parent[label as usize - 1];
        debug_assert_ne!(
            slot.load(Ordering::Relaxed),
            0,
            "label {label} was never made a set"
        );
        slot
    }

    #[inline]
    pub(super) fn label_count(&self) -> usize {
        (self.next_label.load(Ordering::Relaxed) - 1) as usize
    }

    /// Fill `map` with the dense 1..=N relabeling — `map[provisional]` is the final label — and
    /// return N, the number of distinct components.
    ///
    /// Final labels are numbered in the order `provisional` first reaches each component, so a
    /// caller that walks its runs in raster order gets labels independent of how the provisional
    /// ones were handed out across threads. Every provisional label must appear in `provisional`.
    pub(super) fn build_label_map(
        &mut self,
        provisional: impl Iterator<Item = u32>,
        map: &mut Vec<u32>,
    ) -> usize {
        let label_count = self.label_count();
        // `union` links the larger root under the smaller and `make_set` makes a label its own
        // parent, so `parent[l] <= l` throughout: one ascending pass points every label straight at
        // its root, since its parent's entry is already final. The lookups below are then O(1),
        // where walking each chain would be quadratic on a comb-shaped mask.
        let parent = &mut self.parent[..label_count];
        for index in 0..label_count {
            let direct = *parent[index].get_mut() as usize;
            debug_assert!(
                (1..=index + 1).contains(&direct),
                "parent {direct} of label {}",
                index + 1
            );
            let root = *parent[direct - 1].get_mut();
            *parent[index].get_mut() = root;
        }

        map.clear();
        map.resize(label_count + 1, 0);
        let mut count = 0u32;

        for label in provisional {
            let root = *parent[label as usize - 1].get_mut() as usize;
            if map[root] == 0 {
                count += 1;
                map[root] = count;
            }
            map[label as usize] = map[root];
        }

        count as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unions in descending order build the longest chain the linking rule allows: label `l`
    /// under `l − 1`, down to 1. Every label is then one component, numbered 1 by the first label
    /// the walk reaches.
    #[test]
    fn a_descending_chain_resolves_to_one_component() {
        const LABELS: u32 = 1000;
        let mut union_find = UnionFind::default();
        union_find.reset(LABELS as usize);
        for expected in 1..=LABELS {
            assert_eq!(union_find.make_set(), expected);
        }
        for label in (1..LABELS).rev() {
            union_find.union(label + 1, label);
        }
        assert_eq!(union_find.find(LABELS), 1);

        let mut map = Vec::new();
        let count = union_find.build_label_map((1..=LABELS).rev(), &mut map);
        assert_eq!(count, 1);
        assert!(map[1..].iter().all(|&label| label == 1));
    }

    /// Two chains stay two components, numbered in the order the walk first reaches them.
    #[test]
    fn components_are_numbered_by_first_appearance() {
        let mut union_find = UnionFind::default();
        union_find.reset(4);
        for _ in 0..4 {
            union_find.make_set();
        }
        union_find.union(2, 1);
        union_find.union(4, 3);

        let mut map = Vec::new();
        let count = union_find.build_label_map([3, 1, 4, 2].into_iter(), &mut map);
        assert_eq!(count, 2);
        assert_eq!(&map[1..], &[2, 2, 1, 1]);
    }
}
