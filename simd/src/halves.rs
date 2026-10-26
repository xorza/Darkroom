//! [`Halves`]: a vector split in two.

/// A vector split in two, each half widened to twice its lane width: the low lanes and the high.
#[derive(Debug, Clone, Copy)]
pub struct Halves<V> {
    pub low: V,
    pub high: V,
}
