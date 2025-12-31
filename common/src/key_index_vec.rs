use std::hash::Hash;
use std::{collections::HashMap, ops::Index, ops::IndexMut};

use serde::de::Error as SerdeError;
use serde::{Deserialize, Serialize};

use crate::is_debug;

pub trait KeyIndexKey<K> {
    fn key(&self) -> &K;
}

#[derive(Debug, Clone, Default)]
pub struct KeyIndexVec<K: Copy + Eq + Hash, V: Default + KeyIndexKey<K>> {
    pub items: Vec<V>,
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

    pub fn remove_by_key(&mut self, key: &K) -> Option<V> {
        let idx = self.idx_by_key.remove(key)?;
        let removed = self.items.remove(idx);
        assert!(*removed.key() == *key);

        for (pos, item) in self.items.iter().enumerate().skip(idx) {
            self.idx_by_key.insert(*item.key(), pos);
        }

        Some(removed)
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

    pub fn index_of_key(&self, key: &K) -> Option<usize> {
        self.idx_by_key.get(key).copied()
    }

    pub fn by_key(&self, key: &K) -> Option<&V> {
        self.index_of_key(key).map(|idx| &self.items[idx])
    }

    pub fn by_key_mut(&mut self, key: &K) -> Option<&mut V> {
        let idx = self.index_of_key(key)?;
        Some(&mut self.items[idx])
    }

    pub fn compact_insert_with(
        &mut self,
        key: &K,
        write_idx: &mut usize,
        create: impl FnOnce() -> V,
    ) -> usize {
        assert!(*write_idx <= self.items.len());
        let idx = match self.idx_by_key.get(key).copied() {
            Some(idx) => idx,
            None => {
                let value = create();
                assert!(*value.key() == *key);
                let idx = self.items.len();
                self.items.push(value);
                self.idx_by_key.insert(*key, idx);
                idx
            }
        };
        if idx < *write_idx {
            return idx;
        }

        if idx > *write_idx {
            self.items.swap(idx, *write_idx);
            let swapped_key = *self.items[idx].key();
            self.idx_by_key.insert(swapped_key, idx);
        }

        self.idx_by_key.insert(*key, *write_idx);
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
                assert_eq!(idx, self.index_of_key(v.key()).unwrap());
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

impl<K, V> Serialize for KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: Default + KeyIndexKey<K> + Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.items.serialize(serializer)
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
        let items: Vec<V> = Vec::deserialize(deserializer)?;
        let mut idx_by_key = HashMap::with_capacity(items.len());
        for (idx, item) in items.iter().enumerate() {
            let key = *item.key();
            if idx_by_key.insert(key, idx).is_some() {
                return Err(SerdeError::custom("Duplicate key in KeyIndexVec"));
            }
        }

        Ok(Self { items, idx_by_key })
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
                deserialize(&serialized, format).unwrap();

            assert_eq!(deserialized.items.len(), 2);
            assert_eq!(deserialized.idx_by_key.len(), 2);

            let item_a = deserialized.by_key(&1).unwrap();
            assert_eq!(item_a.id, 1);
            assert_eq!(item_a.value, 10);

            let item_b = deserialized.by_key(&2).unwrap();
            assert_eq!(item_b.id, 2);
            assert_eq!(item_b.value, 20);
        }
    }

    #[test]
    fn compact_insert_with_cases() {
        let mut vec = KeyIndexVec::<u32, TestItem>::default();
        vec.push(TestItem { id: 10, value: 100 });
        vec.push(TestItem { id: 20, value: 200 });
        vec.push(TestItem { id: 30, value: 300 });

        let mut write_idx = 0;

        // idx == write_idx
        let idx = vec.compact_insert_with(&10, &mut write_idx, || TestItem { id: 10, value: 0 });
        assert_eq!(idx, 0);
        assert_eq!(write_idx, 1);
        assert_eq!(vec.index_of_key(&10), Some(0));

        // idx > write_idx (swap)
        let idx = vec.compact_insert_with(&30, &mut write_idx, || TestItem { id: 30, value: 0 });
        assert_eq!(idx, 1);
        assert_eq!(write_idx, 2);
        assert_eq!(vec.index_of_key(&30), Some(1));
        assert_eq!(vec.index_of_key(&20), Some(2));

        // idx < write_idx (already compacted)
        let idx = vec.compact_insert_with(&10, &mut write_idx, || TestItem { id: 10, value: 0 });
        assert_eq!(idx, 0);
        assert_eq!(write_idx, 2);

        // new key insert (appends then swaps into write_idx if needed)
        let idx = vec.compact_insert_with(&40, &mut write_idx, || TestItem { id: 40, value: 400 });
        assert_eq!(idx, 2);
        assert_eq!(write_idx, 3);
        assert_eq!(vec.index_of_key(&40), Some(2));

        vec.compact_finish(write_idx);
        assert_eq!(vec.items.len(), 3);
        assert_eq!(vec.idx_by_key.len(), 3);
        assert!(vec.by_key(&20).is_none());
        assert_eq!(vec.by_key(&10).unwrap().value, 100);
        assert_eq!(vec.by_key(&30).unwrap().value, 300);
        assert_eq!(vec.by_key(&40).unwrap().value, 400);
    }
}
