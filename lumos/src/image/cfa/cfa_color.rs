//! [`CfaColor`]: the color of a photosite's filter.

/// The color of a photosite's filter. Its [`index`](Self::index) is its slot in a per-color
/// array: red 0, green 1, blue 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CfaColor {
    Red,
    Green,
    Blue,
}

impl CfaColor {
    /// The color's slot in a per-color array.
    #[inline(always)]
    pub const fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Green => 1,
            Self::Blue => 2,
        }
    }

    /// The color in slot `index`, or `None` past blue.
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Red),
            1 => Some(Self::Green),
            2 => Some(Self::Blue),
            _ => None,
        }
    }
}
