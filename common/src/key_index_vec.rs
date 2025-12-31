use std::hash::Hash;
use std::{collections::HashMap, ops::Index, ops::IndexMut};

use serde::de::Error as SerdeError;
use serde::{Deserialize, Serialize};

use crate::is_debug;

pub trait KeyIndexKey<K> {
    fn key(&self) -> &K;
}

#[derive(Debug, Default, Serialize)]
pub struct KeyIndexVec<K: Copy + Eq + Hash, V: Default + KeyIndexKey<K>> {
    pub items: Vec<V>,
    #[serde(skip)]
    pub idx_by_key: HashMap<K, usize>,
}

impl<K, V> KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: Default + KeyIndexKey<K>,
{
    pub fn push(&mut self, v: V) {
        self.idx_by_key.insert(*v.key(), self.items.len());
        self.items.push(v);
    }

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
        assert!(*write_idx <= self.items.len());
        let idx = self.get_or_insert_default(key);
        if idx < *write_idx {
            return idx;
        }

        if idx > *write_idx {
            self.items.swap(idx, *write_idx);
            let swapped_key = *self.items[idx].key();
            self.idx_by_key.insert(swapped_key, idx);
        }

        self.idx_by_key.insert(key, *write_idx);
        *write_idx += 1;
        *write_idx - 1
    }

    pub fn compact_finish(&mut self, write_idx: usize) {
        assert!(write_idx <= self.items.len());
        self.items.truncate(write_idx);
        self.idx_by_key
            .retain(|&id, &mut idx| idx < write_idx && *self.items[idx].key() == id);
        assert_eq!(self.items.len(), self.idx_by_key.len());

        if is_debug() {
            for (idx, v) in self.items.iter().enumerate() {
                assert_eq!(idx, self.index_of(v.key()).unwrap());
            }
        }
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

impl<'de, K, V> Deserialize<'de> for KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash + Deserialize<'de>,
    V: Default + KeyIndexKey<K> + Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper<V> {
            items: Vec<V>,
        }

        let helper: Helper<V> = Helper::deserialize(deserializer)?;
        let mut idx_by_key = HashMap::with_capacity(helper.items.len());
        for (idx, item) in helper.items.iter().enumerate() {
            let key = *item.key();
            if idx_by_key.insert(key, idx).is_some() {
                return Err(SerdeError::custom("Duplicate key in KeyIndexVec"));
            }
        }

        Ok(Self {
            items: helper.items,
            idx_by_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{deserialize, serialize, FileFormat};

    #[derive(Debug, Default, Serialize, Deserialize)]
    struct TestItem {
        id: u32,
        value: i32,
    }

    impl KeyIndexKey<u32> for TestItem {
        fn key(&self) -> &u32 {
            &self.id
        }
    }

    #[test]
    fn key_index_vec_roundtrip_formats() {
        let mut vec = KeyIndexVec::<u32, TestItem>::default();
        vec.push(TestItem { id: 1, value: 10 });
        vec.push(TestItem { id: 2, value: 20 });

        for format in [FileFormat::Yaml, FileFormat::Json, FileFormat::Lua] {
            let serialized = serialize(&vec, format);
            let deserialized: KeyIndexVec<u32, TestItem> =
                deserialize(&serialized, format).expect("Failed to deserialize KeyIndexVec");

            assert_eq!(deserialized.items.len(), 2);
            assert_eq!(deserialized.idx_by_key.len(), 2);

            let item_a = deserialized.by_key(&1).expect("Missing item for key 1");
            assert_eq!(item_a.id, 1);
            assert_eq!(item_a.value, 10);

            let item_b = deserialized.by_key(&2).expect("Missing item for key 2");
            assert_eq!(item_b.id, 2);
            assert_eq!(item_b.value, 20);
        }
    }
}
