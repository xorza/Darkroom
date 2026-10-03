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

    /// This pixel moved to the intensity `target` with its hue kept, in display range: every
    /// channel scaled by `target / intensity`; if one would pass white, all three divided by the
    /// largest instead of that one clipped, which would shift the hue; a channel below black
    /// clamped to 0. A pixel with no positive intensity has no hue to keep and goes black.
    #[inline]
    pub(crate) fn with_intensity(self, target: f32) -> Rgb {
        let intensity = self.intensity();
        if intensity <= 0.0 {
            return Rgb::ZERO;
        }
        let scaled = self.scale(target / intensity);
        let max = scaled.r.max(scaled.g).max(scaled.b);
        let capped = if max > 1.0 {
            scaled.scale(1.0 / max)
        } else {
            scaled
        };
        Rgb {
            r: capped.r.max(0.0),
            g: capped.g.max(0.0),
            b: capped.b.max(0.0),
        }
    }
}

#[cfg(test)]
mod tests;
