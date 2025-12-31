use std::hash::Hash;
use std::{collections::HashMap, ops::Index, ops::IndexMut};

use serde::{Deserialize, Serialize};

pub trait KeyIndexKey<K> {
    fn key(&self) -> K;
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct KeyIndexVec<K: Copy + Eq + Hash, V: Default + KeyIndexKey<K>> {
    pub items: Vec<V>,
    pub idx_by_key: HashMap<K, usize>,
}

impl<K, V> KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: Default + KeyIndexKey<K>,
{
    pub fn iter(&self) -> std::slice::Iter<'_, V> {
        self.items.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, V> {
        self.items.iter_mut()
    }

    pub fn len(&self) -> usize {
        assert_eq!(self.items.len(), self.idx_by_key.len());
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        assert_eq!(self.items.len(), self.idx_by_key.len());
        self.items.is_empty()
    }

    pub fn index_of(&self, key: &K) -> Option<usize> {
        self.idx_by_key.get(key).copied()
    }

    pub fn by_key(&self, key: &K) -> Option<&V> {
        self.index_of(key).map(|idx| &self.items[idx])
    }

    pub fn by_key_mut(&mut self, key: &K) -> Option<&mut V> {
        let idx = self.index_of(key)?;
        Some(&mut self.items[idx])
    }

    pub fn get_or_insert_default(&mut self, key: K) -> usize {
        if let Some(&idx) = self.idx_by_key.get(&key) {
            return idx;
        }
        let idx = self.items.len();
        self.items.push(V::default());
        self.idx_by_key.insert(key, idx);
        idx
    }

    pub fn compact_insert_default(&mut self, key: K, write_idx: &mut usize) -> usize {
        assert!(
            *write_idx <= self.items.len(),
            "KeyIndexVec compact write index out of bounds: {write_idx} > {}",
            self.items.len()
        );
        let idx = self.get_or_insert_default(key);
        if idx < *write_idx {
            return idx;
        }

        if idx > *write_idx {
            self.items.swap(idx, *write_idx);
            let swapped_key = self.items[idx].key();
            self.idx_by_key.insert(swapped_key, idx);
        }

        self.idx_by_key.insert(key, *write_idx);
        *write_idx += 1;
        *write_idx - 1
    }

    pub fn compact_finish(&mut self, write_idx: usize) {
        assert!(
            write_idx <= self.items.len(),
            "KeyIndexVec compact write index out of bounds: {write_idx} > {}",
            self.items.len()
        );
        self.items.truncate(write_idx);
        self.idx_by_key
            .retain(|&id, &mut idx| idx < write_idx && self.items[idx].key() == id);
        assert_eq!(
            self.items.len(),
            self.idx_by_key.len(),
            "KeyIndexVec invariant violated: items and idx_by_key length mismatch"
        );
    }
}

impl<K, V> Index<usize> for KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: Default + KeyIndexKey<K>,
{
    type Output = V;

    fn index(&self, idx: usize) -> &Self::Output {
        assert!(idx < self.items.len());
        &self.items[idx]
    }
}

impl<K, V> IndexMut<usize> for KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: Default + KeyIndexKey<K>,
{
    fn index_mut(&mut self, idx: usize) -> &mut Self::Output {
        assert!(idx < self.items.len());
        &mut self.items[idx]
    }
}
