//! The disjoint-set structure that resolves provisional run labels into components.

use std::sync::atomic::{AtomicU32, Ordering};

/// Lock-free union-find over provisional run labels.
///
/// Operations take `&self` because the strips share one instance across threads.
pub(super) struct UnionFind {
    parent: Vec<AtomicU32>,
    next_label: AtomicU32,
}

impl std::fmt::Debug for UnionFind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnionFind")
            .field("len", &self.parent.len())
            .field("next_label", &self.next_label.load(Ordering::Relaxed))
            .finish()
    }
}

/// Dense 1..=N relabeling from [`UnionFind::build_label_map`]: `map[provisional]` is the
/// final label, and `count` is the number of distinct components (the max final label).
#[derive(Debug)]
pub(super) struct LabelMapping {
    pub(super) map: Vec<u32>,
    pub(super) count: usize,
}

impl UnionFind {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            parent: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            next_label: AtomicU32::new(1),
        }
    }

    #[inline]
    pub(super) fn make_set(&self) -> u32 {
        // SeqCst: labels must be globally unique across threads.
        let label = self.next_label.fetch_add(1, Ordering::SeqCst);
        assert!(
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
            let idx = (current - 1) as usize;
            if idx >= self.parent.len() {
                return current;
            }
            // Relaxed: find is idempotent — stale reads just cause extra
            // iterations, union's CAS provides the synchronization.
            let parent = self.parent[idx].load(Ordering::Relaxed);
            if parent == current || parent == 0 {
                return current;
            }
            current = parent;
        }
    }

    pub(super) fn union(&self, a: u32, b: u32) {
        let mut root_a = self.find(a);
        let mut root_b = self.find(b);

        while root_a != root_b {
            if root_a > root_b {
                std::mem::swap(&mut root_a, &mut root_b);
            }

            let idx_b = (root_b - 1) as usize;
            if idx_b >= self.parent.len() {
                break;
            }

            // AcqRel: acquire sees prior unions, release publishes this union.
            // Relaxed on failure: we re-find roots anyway.
            match self.parent[idx_b].compare_exchange_weak(
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

    #[inline]
    pub(super) fn label_count(&self) -> usize {
        (self.next_label.load(Ordering::Relaxed) - 1) as usize
    }

    /// Build the dense 1..=N label mapping (single pass) together with the component count.
    pub(super) fn build_label_map(&self, total_labels: usize) -> LabelMapping {
        let mut map = vec![0u32; total_labels + 1];
        let mut count = 0u32;

        for i in 1..=total_labels {
            let root = self.find(i as u32);
            if map[root as usize] == 0 {
                count += 1;
                map[root as usize] = count;
            }
            map[i] = map[root as usize];
        }

        LabelMapping {
            map,
            count: count as usize,
        }
    }
}
