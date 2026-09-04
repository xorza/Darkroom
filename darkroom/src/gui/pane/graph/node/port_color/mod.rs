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
//! other colour); this module owns only the type → slot mapping and the
//! hover emphasis.

use palantir::RgbaF32;
use scenarium::DataType;

use crate::core::document::PortKind;
use crate::gui::theme::Theme;
use crate::gui::theme::color::toward;
use crate::gui::theme::type_colors::TypeColors;

/// RgbaF32 for a port of type `ty` on the given side. Untyped (`Any`) ports
/// defer to the theme's positional port colors; `hovered` lifts either one
/// through [`emphasize`].
pub(crate) fn port_color(theme: &Theme, ty: &DataType, kind: PortKind, hovered: bool) -> RgbaF32 {
    let base = if matches!(ty, DataType::Any) {
        fallback(theme, kind)
    } else {
        type_hue(&theme.type_colors, ty)
    };
    lit(base, hovered)
}

/// RgbaF32 for an event emitter glyph, subscription pin, or event wire.
/// Events carry no data type, so they use the theme's event colour rather
/// than a type hue; `hovered` lifts it like every other port.
pub(crate) fn event_color(theme: &Theme, hovered: bool) -> RgbaF32 {
    lit(theme.ports.event, hovered)
}

fn fallback(theme: &Theme, kind: PortKind) -> RgbaF32 {
    match kind {
        PortKind::Input => theme.ports.input,
        PortKind::Output => theme.ports.output,
    }
}

fn lit(c: RgbaF32, hovered: bool) -> RgbaF32 {
    if hovered { emphasize(c) } else { c }
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
        DataType::Custom(id) if *id == *lens::IMAGE_TYPE_ID => t.image,
        DataType::Custom(id) | DataType::Enum(id) => ramp_pick(&t.ramp, id.as_u128()),
        DataType::Any => unreachable!("Any handled by fallback in port_color"),
    }
}

/// Pick a ramp entry from a type id so a given custom/enum type always
/// lands on the same color.
fn ramp_pick(ramp: &[RgbaF32], key: u128) -> RgbaF32 {
    ramp[(key % ramp.len() as u128) as usize]
}

/// Hover emphasis: blend toward white, so the port lifts off the canvas.
///
/// Every port lifts this way, typed or not. The palette cannot carry a
/// lifted colour beside each resting one — most port colours already sit on
/// its brightest tint, which has nothing above it — so the lift is computed
/// here instead.
fn emphasize(c: RgbaF32) -> RgbaF32 {
    const T: f32 = 0.28;
    toward(c, RgbaF32::WHITE, T)
}

#[cfg(test)]
mod tests;
