//! Maps a port's [`DataType`] to the color its circle (and the wires
//! touching it) paint with, so a graph reads by type at a glance.
//!
//! Built-in scalar types get a fixed hue each, and so does the lens
//! image type — the dominant payload on a darkroom canvas earns a deliberate
//! color, not a hash pick. Remaining `Custom` / `Enum` types are keyed by
//! their `type_id` onto a small ramp, so distinct custom types land on
//! stable, distinct colors without enumerating them here. `Any` (the
//! default / untyped boundary placeholder) has no type identity, so it
//! falls back to the positional input/output port colors from the theme.
//!
//! The hue rosters themselves live on the theme
//! ([`TypeColors`], serialized like every
//! other color), and so does the hover lift; this module owns only the
//! type → slot mapping.

use palantir::RgbaF32;
use scenarium::DataType;

use crate::gui::port_ref::PortKind;
use crate::gui::theme::Theme;
use crate::gui::theme::type_colors::TypeColors;

/// `RgbaF32` for a port of type `ty` on the given side, at rest — see
/// [`Hover::lift`](crate::gui::widgets::hover::Hover::lift). Untyped (`Any`) ports defer to the
/// theme's positional port colors.
pub(crate) fn data(theme: &Theme, ty: &DataType, kind: PortKind) -> RgbaF32 {
    if matches!(ty, DataType::Any) {
        fallback(theme, kind)
    } else {
        type_hue(&theme.type_colors, ty)
    }
}

/// `RgbaF32` for an event emitter glyph, subscription pin, or event wire, at
/// rest. Events carry no data type, so they use the theme's event color
/// rather than a type hue.
pub(crate) const fn event(theme: &Theme) -> RgbaF32 {
    theme.ports.event
}

const fn fallback(theme: &Theme, kind: PortKind) -> RgbaF32 {
    match kind {
        PortKind::Input => theme.ports.input,
        PortKind::Output => theme.ports.output,
    }
}

/// The base hue for a non-`Any` type under the theme's roster.
fn type_hue(t: &TypeColors, ty: &DataType) -> RgbaF32 {
    match ty {
        DataType::Bool => t.boolean,
        DataType::Int => t.int,
        DataType::Float => t.float,
        DataType::String => t.string,
        DataType::FsPath(_) => t.path,
        // Image is the dominant type on a darkroom canvas — it owns a fixed
        // hue instead of a hash pick, so its wires read as one deliberate
        // color (and can't land next to Float or the status purples).
        DataType::Custom(id) if *id == lens::IMAGE_TYPE_ID => t.image,
        DataType::Custom(id) | DataType::Enum(id) => ramp_pick(&t.ramp, id.as_u128()),
        DataType::Any => unreachable!("Any handled by fallback in port_color"),
    }
}

/// Pick a ramp entry from a type id so a given custom/enum type always
/// lands on the same color.
const fn ramp_pick(ramp: &[RgbaF32], key: u128) -> RgbaF32 {
    ramp[(key % ramp.len() as u128) as usize]
}

#[cfg(test)]
mod tests;
