use std::hash::Hash;
use std::{collections::HashMap, ops::Index, ops::IndexMut};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct KeyIndexVec<K: Copy + Eq + Hash, V: Default> {
    pub items: Vec<V>,
    pub idx_by_key: HashMap<K, usize>,
}

impl<K, V> KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: Default,
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
}

impl<K, V> Index<usize> for KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: Default,
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
    V: Default,
{
    fn index_mut(&mut self, idx: usize) -> &mut Self::Output {
        assert!(idx < self.items.len());
        &mut self.items[idx]
    }
}
