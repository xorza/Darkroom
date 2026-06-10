//! A simple RGB color as three `f32` channel values.

/// An RGB color: three `f32` channel values. A small value type for per-pixel color work.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    /// All channels zero (black).
    pub const ZERO: Rgb = Rgb {
        r: 0.0,
        g: 0.0,
        b: 0.0,
    };

    /// Combined intensity — the unweighted channel mean `(r + g + b) / 3`.
    #[inline]
    pub const fn intensity(self) -> f32 {
        (self.r + self.g + self.b) * (1.0 / 3.0)
    }

    /// Scale all three channels by `f`.
    #[inline]
    pub const fn scale(self, f: f32) -> Rgb {
        Rgb {
            r: self.r * f,
            g: self.g * f,
            b: self.b * f,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rgb;

    #[test]
    fn intensity_scale_zero() {
        let c = Rgb {
            r: 0.3,
            g: 0.6,
            b: 0.9,
        };
        assert!((c.intensity() - 0.6).abs() < 1e-6, "(0.3+0.6+0.9)/3 = 0.6");
        let s = c.scale(2.0);
        assert!(
            (s.r - 0.6).abs() < 1e-6 && (s.g - 1.2).abs() < 1e-6 && (s.b - 1.8).abs() < 1e-6,
            "scale by 2: {s:?}"
        );
        assert_eq!(
            Rgb::ZERO,
            Rgb {
                r: 0.0,
                g: 0.0,
                b: 0.0
            }
        );
    }
}
