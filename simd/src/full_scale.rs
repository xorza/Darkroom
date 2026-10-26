//! [`FullScale`]: the full scale of an integer sample type, which a float sample is divided by.

/// The full scale of an integer sample type, which [`Isa::div_full_scale`](crate::Isa) divides a
/// sample by: one of two, so the divisor and its reciprocal cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullScale {
    U8,
    U16,
}

impl FullScale {
    /// The full scale: 255 or 65 535.
    pub const fn divisor(self) -> f32 {
        match self {
            Self::U8 => 255.0,
            Self::U16 => 65535.0,
        }
    }

    /// The divisor's reciprocal, rounded.
    pub const fn reciprocal(self) -> f32 {
        1.0 / self.divisor()
    }
}
