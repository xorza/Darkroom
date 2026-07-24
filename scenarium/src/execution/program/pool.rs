use std::marker::PhantomData;
use std::ops::{Deref, Index, IndexMut};

#[derive(Debug)]
pub(crate) struct PoolRange<T> {
    pub(crate) start: u32,
    pub(crate) len: u32,
    marker: PhantomData<fn() -> T>,
}

impl<T> PoolRange<T> {
    fn new(start: u32, len: u32) -> Self {
        Self {
            start,
            len,
            marker: PhantomData,
        }
    }

    pub(crate) fn range(self) -> std::ops::Range<usize> {
        let start = self.start as usize;
        start..start + self.len as usize
    }
}

impl<T> Clone for PoolRange<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for PoolRange<T> {}

impl<T> Default for PoolRange<T> {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

#[derive(Debug, Default)]
pub(crate) struct Pool<T> {
    values: Vec<T>,
}

impl<T> Pool<T> {
    pub(crate) fn append(&mut self, values: impl IntoIterator<Item = T>) -> PoolRange<T> {
        let start = u32::try_from(self.values.len()).expect("program pool start exceeds u32");
        self.values.extend(values);
        let end = u32::try_from(self.values.len()).expect("program pool length exceeds u32");
        PoolRange::new(start, end - start)
    }

    pub(crate) fn clear(&mut self) {
        self.values.clear();
    }
}

impl<T> Deref for Pool<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.values
    }
}

impl<T> Index<usize> for Pool<T> {
    type Output = T;

    fn index(&self, index: usize) -> &T {
        &self.values[index]
    }
}

impl<T> IndexMut<usize> for Pool<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        &mut self.values[index]
    }
}

impl<T> Index<PoolRange<T>> for Pool<T> {
    type Output = [T];

    fn index(&self, range: PoolRange<T>) -> &[T] {
        &self.values[range.range()]
    }
}

impl<T> IndexMut<PoolRange<T>> for Pool<T> {
    fn index_mut(&mut self, range: PoolRange<T>) -> &mut [T] {
        &mut self.values[range.range()]
    }
}

#[cfg(test)]
mod tests {
    use crate::execution::program::pool::Pool;

    #[test]
    fn append_returns_typed_ranges_into_one_packed_pool() {
        let mut pool = Pool::default();

        let first = pool.append([10, 20]);
        let empty = pool.append([]);
        let second = pool.append([30]);

        assert_eq!(first.start, 0);
        assert_eq!(first.len, 2);
        assert_eq!(empty.start, 2);
        assert_eq!(empty.len, 0);
        assert_eq!(second.start, 2);
        assert_eq!(second.len, 1);
        assert_eq!(&*pool, [10, 20, 30]);
        assert_eq!(pool[0], 10);
        assert_eq!(pool[first], [10, 20]);
        assert!(pool[empty].is_empty());
        assert_eq!(pool[second], [30]);

        pool[0] = 15;
        pool[first][1] = 25;
        assert_eq!(&*pool, [15, 25, 30]);

        let capacity = pool.values.capacity();
        pool.clear();
        assert!(pool.is_empty());
        assert_eq!(pool.values.capacity(), capacity);
        let rebuilt = pool.append([40]);
        assert_eq!(rebuilt.start, 0);
        assert_eq!(rebuilt.len, 1);
        assert_eq!(pool[0], 40);
    }
}
