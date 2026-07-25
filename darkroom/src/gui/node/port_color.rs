//! Maps a port's [`DataType`] to the color its circle (and the wires
//! touching it) paint with, so a graph reads by type at a glance.
//!
//! Built-in scalar types get fixed hues per palette, and so does the lens
//! image type — the dominant payload on a darkroom canvas earns a deliberate
//! color, not a hash pick. Remaining `Custom` / `Enum` types are keyed by
//! their `type_id` onto a small ramp, so distinct custom types land on
//! stable, distinct colors without enumerating them here. `Any` (the
//! default / untyped boundary placeholder) has no type identity, so it
//! falls back to the positional input/output port colors from the theme.
//!
//! The hue rosters themselves live on the theme
//! ([`TypeColors`](crate::gui::theme::TypeColors), serialized like every
//! other swatch); this module owns only the type → slot mapping and the
//! hover emphasis.

use aperture::Color;
use scenarium::DataType;

use crate::core::document::PortKind;
use crate::gui::canvas::wire::toward;
use crate::gui::theme::{Theme, ThemePreset, TypeColors};

/// Color for a port of type `ty` on the given side. `hovered` lightens
/// (dark theme) or darkens (light theme) the typed hue for emphasis;
/// untyped (`Any`) ports defer to the theme's positional port colors,
/// which carry their own hover variants.
pub(crate) fn port_color(theme: &Theme, ty: &DataType, kind: PortKind, hovered: bool) -> Color {
    if matches!(ty, DataType::Any) {
        return fallback(theme, kind, hovered);
    }
    let base = type_hue(&theme.type_colors, ty);
    if hovered {
        emphasize(base, theme.preset)
    } else {
        base
    }
}

/// Color for an event emitter glyph, subscription pin, or event wire.
/// Events carry no data type, so they use the theme's neutral event swatch
/// (not a type hue); `hovered` lifts it like the positional port colors.
pub(crate) fn event_color(theme: &Theme, hovered: bool) -> Color {
    theme.colors.event_port.pick(hovered)
}

/// Positional color for an untyped port — the theme's input/output port
/// swatch, hover variant included.
fn fallback(theme: &Theme, kind: PortKind, hovered: bool) -> Color {
    match kind {
        PortKind::Input => theme.colors.input_port.pick(hovered),
        PortKind::Output => theme.colors.output_port.pick(hovered),
    }
}

/// The base hue for a non-`Any` type under the theme's roster.
fn type_hue(t: &TypeColors, ty: &DataType) -> Color {
    match ty {
        DataType::Bool => t.boolean,
        DataType::Int => t.int,
        DataType::Float => t.float,
        DataType::String => t.string,
        DataType::FsPath(_) => t.path,
        // Image is the dominant type on a darkroom canvas — it owns a fixed
        // hue instead of a hash pick, so its wires read as one deliberate
        // color (and can't land next to Float or the status purples).
        DataType::Custom(id) if *id == *lens::IMAGE_TYPE_ID => t.image,
        DataType::Custom(id) | DataType::Enum(id) => ramp_pick(&t.ramp, id.as_u128()),
        DataType::Any => unreachable!("Any handled by fallback in port_color"),
    }
}

/// Pick a ramp entry from a type id so a given custom/enum type always
/// lands on the same color.
fn ramp_pick(ramp: &[Color], key: u128) -> Color {
    ramp[(key % ramp.len() as u128) as usize]
}

/// Hover emphasis: blend toward white on the dark palette, toward black
/// on the light one, so the port lifts off its canvas either way.
fn emphasize(c: Color, preset: ThemePreset) -> Color {
    const T: f32 = 0.28;
    match preset {
        ThemePreset::Dark => toward(c, Color::WHITE, T),
        ThemePreset::Light => toward(c, Color::BLACK, T),
    }
}

#[cfg(test)]
mod tests {
    use scenarium::TypeId;

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
            port_color(&t, &DataType::Any, PortKind::Input, false),
            t.colors.input_port.rest
        );
        assert_eq!(
            port_color(&t, &DataType::Any, PortKind::Output, false),
            t.colors.output_port.rest
        );
        assert_eq!(
            port_color(&t, &DataType::Any, PortKind::Input, true),
            t.colors.input_port.hover
        );
        assert_eq!(
            port_color(&t, &DataType::Any, PortKind::Output, true),
            t.colors.output_port.hover
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
        // The lens image type bypasses the ramp for its owned hue, which no
        // ramp entry (in either palette) may duplicate — a hash pick must
        // never impersonate the image color.
        let image_ty = DataType::Custom(*lens::IMAGE_TYPE_ID);
        assert_eq!(
            port_color(&t, &image_ty, PortKind::Input, false),
            t.type_colors.image
        );
        let light = Theme::light();
        assert_eq!(
            port_color(&light, &image_ty, PortKind::Input, false),
            light.type_colors.image
        );
        for tc in [&t.type_colors, &light.type_colors] {
            assert!(!tc.ramp.contains(&tc.image));
        }
    }

    #[test]
    fn event_color_is_neutral_and_lifts_on_hover() {
        // Events use the theme's neutral event swatch, distinct from any data
        // hue, and the hover variant differs from rest on both presets.
        for t in [Theme::dark(), Theme::light()] {
            let rest = event_color(&t, false);
            let hov = event_color(&t, true);
            assert_eq!(rest, t.colors.event_port.rest);
            assert_eq!(hov, t.colors.event_port.hover);
            assert_ne!(rest, hov, "hover must visibly differ from rest");
        }
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
