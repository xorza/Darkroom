use std::ops::{Index, IndexMut};

use crate::data::DynamicValue;

#[derive(Debug, Default)]
pub(crate) struct Args(Vec<DynamicValue>);

impl Args {
    pub(crate) fn from_vec<T>(vec: Vec<T>) -> Self
    where
        T: Into<DynamicValue>,
    {
        Args(vec.into_iter().map(|v| v.into()).collect())
    }
    pub(crate) fn resize_and_clear(&mut self, size: usize) {
        self.0.resize(size, DynamicValue::None);
        self.clear();
    }
    pub(crate) fn clear(&mut self) {
        self.0.fill(DynamicValue::None);
    }
    pub(crate) fn as_slice(&self) -> &[DynamicValue] {
        self.0.as_slice()
    }
    pub(crate) fn as_mut_slice(&mut self) -> &mut [DynamicValue] {
        self.0.as_mut_slice()
    }
}
impl Index<usize> for Args {
    type Output = DynamicValue;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}
impl IndexMut<usize> for Args {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}
