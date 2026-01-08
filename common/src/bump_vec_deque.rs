use std::mem::MaybeUninit;

use bumpalo::collections::Vec as BumpVec;

#[derive(Debug)]
pub struct BumpVecDeque<'bump, T: 'bump> {
    // Stores a fixed-capacity ring buffer of uninitialized slots for bump allocation.
    buf: BumpVec<'bump, MaybeUninit<T>>,
    head: usize,
    len: usize,
}

impl<'bump, T: 'bump> BumpVecDeque<'bump, T> {
    pub fn new_in(bump: &'bump bumpalo::Bump) -> Self {
        Self {
            buf: BumpVec::new_in(bump),
            head: 0,
            len: 0,
        }
    }

    pub fn with_capacity_in(capacity: usize, bump: &'bump bumpalo::Bump) -> Self {
        let mut buf = BumpVec::with_capacity_in(capacity, bump);
        for _ in 0..capacity {
            buf.push(MaybeUninit::uninit());
        }

        Self {
            buf,
            head: 0,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn push_back(&mut self, value: T) {
        if self.len == self.capacity() {
            let new_capacity = self.next_capacity();
            self.grow(new_capacity);
        }

        let capacity = self.capacity();
        assert!(capacity > 0);
        let idx = (self.head + self.len) % capacity;
        self.buf[idx].write(value);
        self.len += 1;
        assert!(self.len <= capacity);
    }

    pub fn pop_front(&mut self) -> T {
        assert!(self.len > 0);
        let capacity = self.capacity();
        assert!(capacity > 0);
        let idx = self.head;
        let value = unsafe { self.buf[idx].assume_init_read() };
        self.len -= 1;
        if self.len == 0 {
            self.head = 0;
        } else {
            self.head = (self.head + 1) % capacity;
        }
        value
    }

    pub fn get(&self, index: usize) -> &T {
        assert!(index < self.len);
        let capacity = self.capacity();
        assert!(capacity > 0);
        let idx = (self.head + index) % capacity;
        unsafe { &*self.buf[idx].as_ptr() }
    }

    pub fn get_mut(&mut self, index: usize) -> &mut T {
        assert!(index < self.len);
        let capacity = self.capacity();
        assert!(capacity > 0);
        let idx = (self.head + index) % capacity;
        unsafe { &mut *self.buf[idx].as_mut_ptr() }
    }

    pub fn clear(&mut self) {
        if self.len == 0 {
            return;
        }

        let capacity = self.capacity();
        assert!(capacity > 0);
        for i in 0..self.len {
            let idx = (self.head + i) % capacity;
            unsafe {
                std::ptr::drop_in_place(self.buf[idx].as_mut_ptr());
            }
        }
        self.head = 0;
        self.len = 0;
    }

    fn next_capacity(&self) -> usize {
        let capacity = self.capacity();
        if capacity == 0 { 1 } else { capacity * 2 }
    }

    fn grow(&mut self, new_capacity: usize) {
        let old_capacity = self.capacity();
        assert!(new_capacity > old_capacity);

        let bump = self.buf.bump();
        let mut new_buf = BumpVec::with_capacity_in(new_capacity, bump);
        for _ in 0..new_capacity {
            new_buf.push(MaybeUninit::uninit());
        }

        if self.len > 0 {
            assert!(old_capacity > 0);
        }
        for i in 0..self.len {
            let old_idx = (self.head + i) % old_capacity;
            let value = unsafe { self.buf[old_idx].assume_init_read() };
            new_buf[i].write(value);
        }

        self.buf = new_buf;
        self.head = 0;
        assert_eq!(self.capacity(), new_capacity);
        assert!(self.len <= self.capacity());
    }
}

impl<'bump, T: 'bump> Drop for BumpVecDeque<'bump, T> {
    fn drop(&mut self) {
        if self.len == 0 {
            return;
        }

        let capacity = self.capacity();
        assert!(capacity > 0);
        for i in 0..self.len {
            let idx = (self.head + i) % capacity;
            unsafe {
                std::ptr::drop_in_place(self.buf[idx].as_mut_ptr());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BumpVecDeque;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn push_pop_preserves_order() {
        let bump = bumpalo::Bump::new();
        let mut deque = BumpVecDeque::new_in(&bump);

        deque.push_back(1);
        deque.push_back(2);
        deque.push_back(3);

        assert_eq!(deque.pop_front(), 1);
        assert_eq!(deque.pop_front(), 2);
        assert_eq!(deque.pop_front(), 3);
        assert!(deque.is_empty());
    }

    #[test]
    fn grow_keeps_elements() {
        let bump = bumpalo::Bump::new();
        let mut deque = BumpVecDeque::with_capacity_in(2, &bump);

        deque.push_back(10);
        deque.push_back(20);
        deque.pop_front();
        deque.push_back(30);
        deque.push_back(40);

        assert_eq!(deque.len(), 3);
        assert_eq!(deque.get(0), &20);
        assert_eq!(deque.get(1), &30);
        assert_eq!(deque.get(2), &40);
        assert_eq!(deque.pop_front(), 20);
        assert_eq!(deque.pop_front(), 30);
        assert_eq!(deque.pop_front(), 40);
        assert!(deque.is_empty());
    }

    #[test]
    fn clear_drops_and_resets() {
        let bump = bumpalo::Bump::new();
        let mut deque = BumpVecDeque::new_in(&bump);

        deque.push_back(7);
        deque.push_back(8);
        deque.clear();

        assert!(deque.is_empty());
        deque.push_back(9);
        assert_eq!(deque.pop_front(), 9);
    }

    #[test]
    fn drop_drops_all_items() {
        #[derive(Debug)]
        struct DropCounter {
            drops: Arc<AtomicUsize>,
        }

        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        {
            let bump = bumpalo::Bump::new();
            let mut deque = BumpVecDeque::new_in(&bump);
            deque.push_back(DropCounter {
                drops: drops.clone(),
            });
            deque.push_back(DropCounter {
                drops: drops.clone(),
            });
            deque.push_back(DropCounter {
                drops: drops.clone(),
            });
            assert_eq!(deque.len(), 3);
        }

        assert_eq!(drops.load(Ordering::SeqCst), 3);
    }
}
