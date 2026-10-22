//! [`CfaColor`]: the color of a photosite's filter.

use std::fmt;

/// The color of a photosite's filter. Its [`idx`](Self::idx) is its slot in a per-color
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
    pub const fn idx(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Green => 1,
            Self::Blue => 2,
        }
    }

    /// The color in slot `idx`, or `None` past blue.
    pub const fn from_idx(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Self::Red),
            1 => Some(Self::Green),
            2 => Some(Self::Blue),
            _ => None,
        }
    }
}

impl fmt::Display for CfaColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Red => "red",
            Self::Green => "green",
            Self::Blue => "blue",
        })
    }
}
