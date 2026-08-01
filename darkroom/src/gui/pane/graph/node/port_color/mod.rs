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
//! ([`TypeColors`], serialized like every
//! other swatch); this module owns only the type → slot mapping and the
//! hover emphasis.

use palantir::Color;
use scenarium::DataType;

use crate::core::document::PortKind;
use crate::gui::theme::color::toward;
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
    theme.ports.event.pick(hovered)
}

/// Positional color for an untyped port — the theme's input/output port
/// swatch, hover variant included.
fn fallback(theme: &Theme, kind: PortKind, hovered: bool) -> Color {
    match kind {
        PortKind::Input => theme.ports.input.pick(hovered),
        PortKind::Output => theme.ports.output.pick(hovered),
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
mod tests;
