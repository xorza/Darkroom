//! [`Frexp`]: a mantissa and an exponent, split from each lane.

/// [`F32x8::frexp`](crate::f32x8::F32x8::frexp)'s two parts, the exponent as an f32.
#[derive(Debug, Clone, Copy)]
pub struct Frexp<V> {
    pub mantissa: V,
    pub exponent: V,
}
