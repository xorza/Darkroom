//! Maps a port's [`DataType`] to the color its circle (and the wires
//! touching it) paint with, so a graph reads by type at a glance.
//!
//! Built-in scalar types get fixed hues per palette; `Custom` / `Enum`
//! types are keyed by their `type_id` onto a small ramp, so distinct
//! custom types (e.g. an image vs. a calibration-masters payload) land on
//! stable, distinct colors without enumerating them here. `Null` (the
//! default / untyped boundary placeholder) has no type identity, so it
//! falls back to the positional input/output port colors from the theme.

use palantir::Color;
use scenarium::data::DataType;

use crate::gui::PortKind;
use crate::gui::theme::{Theme, ThemePreset};

/// Color for a port of type `ty` on the given side. `hovered` lightens
/// (dark theme) or darkens (light theme) the typed hue for emphasis;
/// untyped (`Null`) ports defer to the theme's positional port colors,
/// which carry their own hover variants.
pub(crate) fn port_color(theme: &Theme, ty: &DataType, kind: PortKind, hovered: bool) -> Color {
    if matches!(ty, DataType::Null) {
        return fallback(theme, kind, hovered);
    }
    let base = type_hue(theme.preset, ty);
    if hovered {
        emphasize(base, theme.preset)
    } else {
        base
    }
}

/// Positional color for an untyped port — the theme's input/output port
/// swatch, hover variant included.
fn fallback(theme: &Theme, kind: PortKind, hovered: bool) -> Color {
    match (kind, hovered) {
        (PortKind::Input, false) => theme.input_port,
        (PortKind::Input, true) => theme.input_port_hover,
        (PortKind::Output, false) => theme.output_port,
        (PortKind::Output, true) => theme.output_port_hover,
    }
}

/// The base hue for a non-`Null` type under the given palette.
fn type_hue(preset: ThemePreset, ty: &DataType) -> Color {
    let p = match preset {
        ThemePreset::Dark => &DARK,
        ThemePreset::Light => &LIGHT,
    };
    let hex = match ty {
        DataType::Bool => p.boolean,
        DataType::Int => p.int,
        DataType::Float => p.float,
        DataType::String => p.string,
        DataType::FsPath(_) => p.path,
        DataType::Custom(id) | DataType::Enum(id) => ramp_pick(p.ramp, id.as_u128()),
        DataType::Null => unreachable!("Null handled by fallback in port_color"),
    };
    Color::hex(hex)
}

/// Pick a ramp entry from a type id so a given custom/enum type always
/// lands on the same color.
fn ramp_pick(ramp: &[u32], key: u128) -> u32 {
    ramp[(key % ramp.len() as u128) as usize]
}

/// Hover emphasis: blend toward white on the dark palette, toward black
/// on the light one, so the port lifts off its canvas either way.
fn emphasize(c: Color, preset: ThemePreset) -> Color {
    const T: f32 = 0.28;
    match preset {
        ThemePreset::Dark => mix(c, Color::WHITE, T),
        ThemePreset::Light => mix(c, Color::BLACK, T),
    }
}

/// Linear-space lerp between two colors, preserving `a`'s alpha. Both
/// inputs are already linear (storage form), so a straight component lerp
/// is the correct blend.
fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::linear_rgba(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a,
    )
}

/// A per-palette data-type color set. Hand-tuned to harmonize with the
/// Ayu palettes; centralized here so retuning is a one-line edit. `ramp`
/// backs the open-ended `Custom`/`Enum` families.
struct TypePalette {
    boolean: u32,
    int: u32,
    float: u32,
    string: u32,
    path: u32,
    ramp: &'static [u32],
}

const DARK: TypePalette = TypePalette {
    boolean: 0xf28779,
    int: 0x95e6cb,
    float: 0x73d0ff,
    string: 0xffd173,
    path: 0xd4bfff,
    ramp: &[0xffa759, 0x7bd88f, 0xff9eb5, 0x5ccfe6, 0xcaa9fa, 0xe6cd8a],
};

const LIGHT: TypePalette = TypePalette {
    boolean: 0xe05252,
    int: 0x2e9e5b,
    float: 0x2b8fd6,
    string: 0xb8860b,
    path: 0x7a4fd0,
    ramp: &[0xd9722a, 0x1f8fb3, 0xc23b73, 0x2f9e6a, 0x8b5cf0, 0xa67c1a],
};

#[cfg(test)]
mod tests {
    use scenarium::data::TypeId;

    use super::*;

    fn custom(id: u128) -> DataType {
        DataType::Custom(TypeId::from_u128(id))
    }

    #[test]
    fn distinct_builtin_types_get_distinct_colors() {
        let t = Theme::dark();
        let f = port_color(&t, &DataType::Float, PortKind::Input, false);
        let i = port_color(&t, &DataType::Int, PortKind::Input, false);
        let b = port_color(&t, &DataType::Bool, PortKind::Input, false);
        let s = port_color(&t, &DataType::String, PortKind::Input, false);
        assert_ne!(f, i);
        assert_ne!(i, b);
        assert_ne!(b, s);
        assert_ne!(f, s);
    }

    #[test]
    fn type_color_independent_of_kind() {
        let t = Theme::dark();
        assert_eq!(
            port_color(&t, &DataType::Float, PortKind::Input, false),
            port_color(&t, &DataType::Float, PortKind::Output, false),
        );
    }

    #[test]
    fn null_falls_back_to_positional_port_color() {
        let t = Theme::dark();
        assert_eq!(
            port_color(&t, &DataType::Null, PortKind::Input, false),
            t.input_port
        );
        assert_eq!(
            port_color(&t, &DataType::Null, PortKind::Output, false),
            t.output_port
        );
        assert_eq!(
            port_color(&t, &DataType::Null, PortKind::Input, true),
            t.input_port_hover
        );
        assert_eq!(
            port_color(&t, &DataType::Null, PortKind::Output, true),
            t.output_port_hover
        );
    }

    #[test]
    fn hover_changes_typed_color() {
        let t = Theme::dark();
        let base = port_color(&t, &DataType::Float, PortKind::Input, false);
        let hov = port_color(&t, &DataType::Float, PortKind::Input, true);
        assert_ne!(base, hov);
    }

    #[test]
    fn custom_types_keyed_by_type_id() {
        let t = Theme::dark();
        // Same id → same color.
        assert_eq!(
            port_color(&t, &custom(7), PortKind::Input, false),
            port_color(&t, &custom(7), PortKind::Input, false),
        );
        // Ids in adjacent ramp slots → different colors (ramp entries
        // are distinct, len > 1).
        assert_ne!(
            port_color(&t, &custom(0), PortKind::Input, false),
            port_color(&t, &custom(1), PortKind::Input, false),
        );
    }

    #[test]
    fn light_and_dark_palettes_differ() {
        let dark = Theme::dark();
        let light = Theme::light();
        assert_ne!(
            port_color(&dark, &DataType::Float, PortKind::Input, false),
            port_color(&light, &DataType::Float, PortKind::Input, false),
        );
    }
}
