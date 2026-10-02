//! [`DataShape`]: a named way of filling a row of samples.

use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;

/// A named way of filling a row. `seed` lets a caller draw several decorrelated rows of the same
/// shape, which is what the three-row kernels need.
pub(crate) struct DataShape {
    pub(crate) name: &'static str,
    pub(crate) fill: fn(usize, usize) -> Vec<f32>,
}

impl DataShape {
    /// One row of `width` samples; `seed` shifts the pattern without changing its character.
    pub(crate) fn row(&self, width: usize, seed: usize) -> Vec<f32> {
        (self.fill)(width, seed)
    }
}

impl Debug for DataShape {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("DataShape")
            .field("name", &self.name)
            .finish()
    }
}
