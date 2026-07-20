//! A simple RGB color as three `f32` channel values.

/// An RGB color: three `f32` channel values. A small value type for per-pixel color work.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    /// All channels zero (black).
    pub(crate) const ZERO: Rgb = Rgb {
        r: 0.0,
        g: 0.0,
        b: 0.0,
    };

    /// Combined intensity — the unweighted channel mean `(r + g + b) / 3`.
    #[inline]
    pub(crate) const fn intensity(self) -> f32 {
        (self.r + self.g + self.b) * (1.0 / 3.0)
    }

    /// Scale all three channels by `f`.
    #[inline]
    pub(crate) const fn scale(self, f: f32) -> Rgb {
        Rgb {
            r: self.r * f,
            g: self.g * f,
            b: self.b * f,
        }
    }
}

#[cfg(test)]
mod tests;
