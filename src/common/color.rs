//! The color drawing operations take.

use crate::common::luma;

/// An RGB color with components on `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Color {
    pub const GREEN: Color = Color::rgb(0.0, 1.0, 0.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    /// The Rec. 709 luminance, for drawing on a grey image.
    pub(crate) fn luminance(&self) -> f32 {
        let [wr, wg, wb] = luma::WEIGHTS_F32;
        wr * self.r + wg * self.g + wb * self.b
    }
}

#[cfg(test)]
mod tests {
    use crate::common::color::Color;

    /// `0.2126 · 0.5 + 0.7152 · 0.25 + 0.0722 · 1` in `f32`, summed left to right.
    #[test]
    fn luminance_is_the_rec709_sum() {
        let color = Color::rgb(0.5, 0.25, 1.0);
        assert_eq!(
            color.luminance(),
            0.2126f32 * 0.5 + 0.7152 * 0.25 + 0.0722 * 1.0
        );
        assert_eq!(Color::GREEN.luminance(), 0.7152);
    }
}
