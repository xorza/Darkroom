//! RGBA color type for drawing operations.

/// RGBA color with f32 components in range [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    /// Create a new color from RGBA components.
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Create a new opaque color from RGB components.
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Create a color from RGBA u8 values (0-255).
    pub fn from_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    /// Create an opaque color from RGB u8 values (0-255).
    pub fn from_rgb_u8(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    /// Convert to luminance (grayscale) using Rec. 709 weights.
    pub fn luminance(&self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    /// Convert to RGB array (ignores alpha).
    pub fn to_rgb(&self) -> [f32; 3] {
        [self.r, self.g, self.b]
    }

    /// Convert to RGBA array.
    pub fn to_rgba(&self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Return a new color with modified alpha.
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    // Common colors (opaque)
    pub const RED: Color = Color::rgb(1.0, 0.0, 0.0);
    pub const GREEN: Color = Color::rgb(0.0, 1.0, 0.0);
    pub const BLUE: Color = Color::rgb(0.0, 0.0, 1.0);
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::rgb(0.0, 0.0, 0.0);
    pub const YELLOW: Color = Color::rgb(1.0, 1.0, 0.0);
    pub const CYAN: Color = Color::rgb(0.0, 1.0, 1.0);
    pub const MAGENTA: Color = Color::rgb(1.0, 0.0, 1.0);
    pub const ORANGE: Color = Color::rgb(1.0, 0.5, 0.0);
    pub const TRANSPARENT: Color = Color::new(0.0, 0.0, 0.0, 0.0);
}

impl From<[f32; 3]> for Color {
    fn from(arr: [f32; 3]) -> Self {
        Self {
            r: arr[0],
            g: arr[1],
            b: arr[2],
            a: 1.0,
        }
    }
}

impl From<[f32; 4]> for Color {
    fn from(arr: [f32; 4]) -> Self {
        Self {
            r: arr[0],
            g: arr[1],
            b: arr[2],
            a: arr[3],
        }
    }
}

impl From<Color> for [f32; 3] {
    fn from(c: Color) -> Self {
        [c.r, c.g, c.b]
    }
}

impl From<Color> for [f32; 4] {
    fn from(c: Color) -> Self {
        [c.r, c.g, c.b, c.a]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_new() {
        let c = Color::new(0.5, 0.25, 0.75, 0.5);
        assert_eq!(c.r, 0.5);
        assert_eq!(c.g, 0.25);
        assert_eq!(c.b, 0.75);
        assert_eq!(c.a, 0.5);
    }

    #[test]
    fn test_color_rgb() {
        let c = Color::rgb(0.5, 0.25, 0.75);
        assert_eq!(c.r, 0.5);
        assert_eq!(c.g, 0.25);
        assert_eq!(c.b, 0.75);
        assert_eq!(c.a, 1.0);
    }

    #[test]
    fn test_color_from_u8() {
        let c = Color::from_u8(255, 128, 0, 128);
        assert!((c.r - 1.0).abs() < 0.01);
        assert!((c.g - 0.502).abs() < 0.01);
        assert_eq!(c.b, 0.0);
        assert!((c.a - 0.502).abs() < 0.01);
    }

    #[test]
    fn test_color_from_rgb_u8() {
        let c = Color::from_rgb_u8(255, 128, 0);
        assert!((c.r - 1.0).abs() < 0.01);
        assert!((c.g - 0.502).abs() < 0.01);
        assert_eq!(c.b, 0.0);
        assert_eq!(c.a, 1.0);
    }

    #[test]
    fn test_luminance() {
        assert!((Color::WHITE.luminance() - 1.0).abs() < 0.001);
        assert_eq!(Color::BLACK.luminance(), 0.0);
        // Pure green should have highest luminance contribution
        assert!(Color::GREEN.luminance() > Color::RED.luminance());
        assert!(Color::GREEN.luminance() > Color::BLUE.luminance());
    }

    #[test]
    fn test_to_rgb() {
        let c = Color::new(0.1, 0.2, 0.3, 0.5);
        assert_eq!(c.to_rgb(), [0.1, 0.2, 0.3]);
    }

    #[test]
    fn test_to_rgba() {
        let c = Color::new(0.1, 0.2, 0.3, 0.4);
        assert_eq!(c.to_rgba(), [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn test_with_alpha() {
        let c = Color::RED.with_alpha(0.5);
        assert_eq!(c.r, 1.0);
        assert_eq!(c.g, 0.0);
        assert_eq!(c.b, 0.0);
        assert_eq!(c.a, 0.5);
    }

    #[test]
    fn test_from_array_rgb() {
        let c: Color = [0.1, 0.2, 0.3].into();
        assert_eq!(c.r, 0.1);
        assert_eq!(c.g, 0.2);
        assert_eq!(c.b, 0.3);
        assert_eq!(c.a, 1.0);
    }

    #[test]
    fn test_from_array_rgba() {
        let c: Color = [0.1, 0.2, 0.3, 0.4].into();
        assert_eq!(c.r, 0.1);
        assert_eq!(c.g, 0.2);
        assert_eq!(c.b, 0.3);
        assert_eq!(c.a, 0.4);
    }
}
