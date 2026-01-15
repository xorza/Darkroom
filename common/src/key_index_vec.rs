use std::hash::Hash;
use std::{collections::HashMap, ops::Index, ops::IndexMut};

use serde::de::Error as SerdeError;
use serde::{Deserialize, Serialize};

use crate::is_debug;

pub trait KeyIndexKey<K> {
    fn key(&self) -> &K;
}

#[derive(Debug, Clone)]
pub struct KeyIndexVec<K: Copy + Eq + Hash, V: KeyIndexKey<K>> {
    pub items: Vec<V>,
    pub idx_by_key: HashMap<K, usize>,
}

impl<K, V> PartialEq for KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K> + Eq,
{
    fn eq(&self, other: &Self) -> bool {
        assert_eq!(
            self.items.len(),
            self.idx_by_key.len(),
            "KeyIndexVec must keep items and index map in sync"
        );
        assert_eq!(
            other.items.len(),
            other.idx_by_key.len(),
            "KeyIndexVec must keep items and index map in sync"
        );
        if self.items.len() != other.items.len() {
            return false;
        }

        for item in &self.items {
            let key = item.key();
            let other_item = other.by_key(key);
            if other_item.is_none() {
                return false;
            }
            if item != other_item.unwrap() {
                return false;
            }
        }

        true
    }
}

impl<K, V> Eq for KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K> + Eq,
{
}

impl<K: Copy + Eq + Hash, V: KeyIndexKey<K>> Default for KeyIndexVec<K, V> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            idx_by_key: HashMap::new(),
        }
    }
}

impl<K, V> KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K>,
{
    pub fn compact_insert_start(&mut self) -> CompactInsert<'_, K, V> {
        CompactInsert::new(self)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            items: Vec::with_capacity(capacity),
            idx_by_key: HashMap::with_capacity(capacity),
        }
    }

    pub fn add(&mut self, v: V) {
        let key = *v.key();
        if let Some(&idx) = self.idx_by_key.get(&key) {
            self.items[idx] = v;
        } else {
            self.idx_by_key.insert(key, self.items.len());
            self.items.push(v);
        }
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

    pub fn remove_by_index(&mut self, idx: usize) -> V {
        assert!(
            idx < self.items.len(),
            "remove_by_index expects a valid index"
        );
        let removed = self.items.remove(idx);
        let key = *removed.key();
        let had_key = self.idx_by_key.remove(&key).is_some();
        assert!(had_key, "remove_by_index expects the key to exist");

        for (pos, item) in self.items.iter().enumerate().skip(idx) {
            self.idx_by_key.insert(*item.key(), pos);
        }

        removed
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.idx_by_key.clear();
    }

    pub fn retain(&mut self, mut predicate: impl FnMut(&V) -> bool) {
        self.items.retain(|item| predicate(item));
        self.idx_by_key.clear();
        self.idx_by_key.reserve(self.items.len());
        for (idx, item) in self.items.iter().enumerate() {
            let key = *item.key();
            let replaced = self.idx_by_key.insert(key, idx);
            assert!(replaced.is_none(), "retain cannot produce duplicate keys");
        }
        assert_eq!(
            self.items.len(),
            self.idx_by_key.len(),
            "retain must keep key/index maps in sync"
        );
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

    fn compact_insert_with(
        &mut self,
        key: &K,
        write_idx: &mut usize,
        create: impl FnOnce() -> V,
    ) -> usize {
        assert!(*write_idx <= self.items.len());
        let idx = match self.idx_by_key.get(key).copied() {
            Some(idx) => idx,
            None => {
                let idx = self.items.len();
                let value = create();
                assert!(*value.key() == *key);
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

    fn compact_finish(&mut self, write_idx: usize) {
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

#[derive(Debug)]
pub struct CompactInsert<'a, K: Copy + Eq + Hash, V: KeyIndexKey<K>> {
    vec: &'a mut KeyIndexVec<K, V>,
    write_idx: usize,
    finished: bool,
}

impl<'a, K, V> CompactInsert<'a, K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K>,
{
    fn new(vec: &'a mut KeyIndexVec<K, V>) -> Self {
        Self {
            vec,
            write_idx: 0,
            finished: false,
        }
    }

    pub fn insert_with(&mut self, key: &K, create: impl FnOnce() -> V) -> (usize, &mut V) {
        let idx = self
            .vec
            .compact_insert_with(key, &mut self.write_idx, create);
        assert!(
            idx < self.vec.items.len(),
            "compact insert index out of range"
        );
        let item = &mut self.vec.items[idx];
        (idx, item)
    }

    fn validate_index(&self, idx: usize) {
        assert!(idx < self.write_idx, "compact insert index out of range");
    }
}

impl<K, V> Drop for CompactInsert<'_, K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K>,
{
    fn drop(&mut self) {
        if !self.finished {
            self.vec.compact_finish(self.write_idx);
        }
    }
}

impl<K, V> Index<usize> for CompactInsert<'_, K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K>,
{
    type Output = V;

    fn index(&self, idx: usize) -> &Self::Output {
        self.validate_index(idx);
        &self.vec.items[idx]
    }
}

impl<K, V> IndexMut<usize> for CompactInsert<'_, K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K>,
{
    fn index_mut(&mut self, idx: usize) -> &mut Self::Output {
        self.validate_index(idx);
        &mut self.vec.items[idx]
    }
}

impl<K, V> Index<usize> for KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K>,
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
    V: KeyIndexKey<K>,
{
    fn index_mut(&mut self, idx: usize) -> &mut Self::Output {
        assert!(idx < self.items.len());
        &mut self.items[idx]
    }
}

impl<'a, K, V> IntoIterator for &'a KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K>,
{
    type Item = &'a V;
    type IntoIter = std::slice::Iter<'a, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<'a, K, V> IntoIterator for &'a mut KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K>,
{
    type Item = &'a mut V;
    type IntoIter = std::slice::IterMut<'a, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter_mut()
    }
}

impl<K, V> Serialize for KeyIndexVec<K, V>
where
    K: Copy + Eq + Hash,
    V: KeyIndexKey<K> + Serialize,
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
    V: KeyIndexKey<K> + Deserialize<'de>,
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
    use crate::{SerdeFormat, deserialize, serialize};

    #[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
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
    fn add_overwrites_existing_key() {
        let mut vec = KeyIndexVec::<u32, TestItem>::default();
        vec.add(TestItem { id: 1, value: 10 });
        vec.add(TestItem { id: 1, value: 33 });
        vec.add(TestItem { id: 2, value: 299 });
        vec.add(TestItem { id: 1, value: 99 });

        assert_eq!(vec.items.len(), 2);
        assert_eq!(vec.idx_by_key.len(), 2);
        assert_eq!(vec.index_of_key(&1), Some(0));
        assert_eq!(vec.by_key(&1).unwrap().value, 99);
        assert_eq!(vec.by_key(&2).unwrap().value, 299);
    }

    #[test]
    fn key_index_vec_roundtrip_formats() {
        let mut vec = KeyIndexVec::<u32, TestItem>::default();
        vec.add(TestItem { id: 1, value: 10 });
        vec.add(TestItem { id: 2, value: 20 });

        for format in SerdeFormat::all_formats_for_testing() {
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
        vec.add(TestItem { id: 10, value: 100 });
        vec.add(TestItem { id: 20, value: 200 });
        vec.add(TestItem { id: 30, value: 300 });

        let mut write_idx = 0;

        // idx == write_idx
        let idx = vec.compact_insert_with(&10, &mut write_idx, || TestItem { id: 10, value: 0 });
        assert_eq!(vec.items[idx].id, 10);
        assert_eq!(write_idx, 1);
        assert_eq!(vec.index_of_key(&10), Some(0));

        // idx > write_idx (swap)
        let idx = vec.compact_insert_with(&30, &mut write_idx, || TestItem { id: 30, value: 0 });
        assert_eq!(vec.items[idx].id, 30);
        assert_eq!(write_idx, 2);
        assert_eq!(vec.index_of_key(&30), Some(1));
        assert_eq!(vec.index_of_key(&20), Some(2));

        // idx < write_idx (already compacted)
        let idx = vec.compact_insert_with(&10, &mut write_idx, || TestItem { id: 10, value: 0 });
        assert_eq!(vec.items[idx].id, 10);
        assert_eq!(write_idx, 2);

        // new key insert (appends then swaps into write_idx if needed)
        let idx = vec.compact_insert_with(&40, &mut write_idx, || TestItem { id: 40, value: 400 });
        assert_eq!(vec.items[idx].id, 40);
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

    #[test]
    fn compact_insert_start_finishes_on_drop() {
        let mut vec = KeyIndexVec::<u32, TestItem>::default();
        vec.add(TestItem { id: 10, value: 100 });
        vec.add(TestItem { id: 20, value: 200 });
        vec.add(TestItem { id: 30, value: 300 });

        {
            let mut compact = vec.compact_insert_start();
            let (_idx, _item) = compact.insert_with(&20, || TestItem { id: 20, value: 0 });
        }

        assert_eq!(vec.items.len(), 1);
        assert_eq!(vec.idx_by_key.len(), 1);
        assert_eq!(vec.index_of_key(&20), Some(0));
        assert!(vec.by_key(&10).is_none());
        assert!(vec.by_key(&30).is_none());
    }

    #[test]
    fn compact_insert_start_index_access() {
        let mut vec = KeyIndexVec::<u32, TestItem>::default();
        vec.add(TestItem { id: 1, value: 10 });
        vec.add(TestItem { id: 2, value: 20 });

        {
            let mut compact = vec.compact_insert_start();
            let (idx, _item) = compact.insert_with(&2, || TestItem { id: 2, value: 0 });
            assert_eq!(compact[idx].id, 2);
            compact[idx].value = 33;
        }

        assert_eq!(vec.items.len(), 1);
        assert_eq!(vec.idx_by_key.len(), 1);
        assert_eq!(vec.by_key(&2).unwrap().value, 33);
    }

    #[test]
    #[should_panic(expected = "compact insert index out of range")]
    fn compact_insert_start_index_panics_after_write_idx() {
        let mut vec = KeyIndexVec::<u32, TestItem>::default();
        vec.add(TestItem { id: 1, value: 10 });

        let mut compact = vec.compact_insert_start();
        let (idx, _item) = compact.insert_with(&1, || TestItem { id: 1, value: 0 });
        assert_eq!(idx, 0);

        let item = &compact[1];
        println!("Inserted item {:?}", item);
    }

    #[test]
    fn key_index_vec_eq_is_order_independent() {
        let mut left = KeyIndexVec::<u32, TestItem>::default();
        left.add(TestItem { id: 1, value: 10 });
        left.add(TestItem { id: 2, value: 20 });
        left.add(TestItem { id: 3, value: 30 });

        let mut right = KeyIndexVec::<u32, TestItem>::default();
        right.add(TestItem { id: 3, value: 30 });
        right.add(TestItem { id: 1, value: 10 });
        right.add(TestItem { id: 2, value: 20 });

        assert_eq!(left, right);

        right.add(TestItem { id: 2, value: 99 });
        assert_ne!(left, right);
    }
}
