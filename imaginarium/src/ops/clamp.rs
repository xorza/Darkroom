//! [`Clamp`]: which ends of the nominal range an op holds an `f32` result to.

use std::str::FromStr;

/// Which ends of the nominal `[0, max]` range an op holds its result to, as Nuke's Grade node
/// offers a black and a white clamp. An integer store saturates to the range whatever the choice;
/// an `f32` store keeps what the clamp leaves.
///
/// [`Self::DEFAULT`] is [`Self::Black`], as Nuke's Grade has it: a value below black is no light,
/// and a background an op takes below it takes that much from every pixel a later Add meets. A
/// value past white stays, as a linear image holds its highlights there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Clamp {
    /// The result whole, below black and past white.
    None,
    /// The result held at black from below.
    Black,
    /// The result held to `[black, white]`.
    BlackAndWhite,
}

impl Clamp {
    /// The clamp an op takes unless told otherwise.
    pub const DEFAULT: Self = Self::Black;

    /// Every clamp, in the picker's order.
    pub const ALL: [Self; 3] = [Self::None, Self::Black, Self::BlackAndWhite];

    /// The name a document saves and a picker shows.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Black => "Black",
            Self::BlackAndWhite => "Black and White",
        }
    }

    /// The least result the clamp keeps: black, or `−∞` where it holds no black.
    pub(crate) const fn low(self) -> f32 {
        match self {
            Self::None => f32::NEG_INFINITY,
            Self::Black | Self::BlackAndWhite => 0.0,
        }
    }

    /// The greatest result the clamp keeps over a range whose white is `max`: `max`, or `+∞` where
    /// it holds no white.
    pub(crate) const fn high(self, max: f32) -> f32 {
        match self {
            Self::None | Self::Black => f32::INFINITY,
            Self::BlackAndWhite => max,
        }
    }

    /// The number `packed.wgsl`'s `hold` takes: how many ends the clamp holds.
    #[cfg(feature = "wgpu")]
    pub(crate) const fn shader_code(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Black => 1,
            Self::BlackAndWhite => 2,
        }
    }
}

impl Default for Clamp {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A name that is no clamp's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no clamp has this name")]
pub struct UnknownClamp;

impl FromStr for Clamp {
    type Err = UnknownClamp;

    /// One of [`Clamp::name`]'s names.
    fn from_str(name: &str) -> Result<Self, UnknownClamp> {
        Self::ALL
            .into_iter()
            .find(|clamp| clamp.name() == name)
            .ok_or(UnknownClamp)
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    /// `value` held to `[low, high]` by compare-selects, `low > v ? low : v` and then
    /// `v > high ? high : v`: the scalar references' hold, which keeps a NaN and a signed zero as
    /// `simd`'s `max` and `min` keep them.
    #[inline]
    pub(crate) fn hold(value: f32, low: f32, high: f32) -> f32 {
        let value = if low > value { low } else { value };
        if value > high { high } else { value }
    }
}

#[cfg(test)]
mod tests {
    use crate::ops::clamp::{Clamp, UnknownClamp};

    /// The names documents hold, each parsing back to its clamp, in the picker's order; another
    /// name, a case changed, is refused.
    #[test]
    fn each_name_parses_back_to_its_clamp() {
        assert_eq!(
            Clamp::ALL.map(Clamp::name),
            ["None", "Black", "Black and White"]
        );
        for clamp in Clamp::ALL {
            assert_eq!(clamp.name().parse(), Ok(clamp));
        }
        assert_eq!("black".parse::<Clamp>(), Err(UnknownClamp));
    }
}
