//! [`TypeColors`]: the per-data-type ink roster.

use palantir::RgbaF32;

/// Data-type → wire/port-circle hue roster (consumed by
/// `gui::pane::graph::node::port_color`). Serialized as the theme's
/// `[type_colors]` table so a loaded theme file can restyle type hues like
/// any other colour. `ramp` backs the open-ended `Custom`/`Enum` families —
/// keyed by `type_id`, so distinct custom types land on stable,
/// distinct colors; `image` is the fixed hue the lens image type owns.
///
/// The one type read from both on-disk formats: it is also a field of
/// [`Palette`](crate::gui::theme::palette::Palette), so its shape has to
/// suit RON too. `ramp` is a fixed-size array, which serde reads as a
/// tuple, so the palette file writes it in parentheses and not brackets.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TypeColors {
    pub(crate) boolean: RgbaF32,
    pub(crate) int: RgbaF32,
    pub(crate) float: RgbaF32,
    pub(crate) string: RgbaF32,
    pub(crate) path: RgbaF32,
    pub(crate) image: RgbaF32,
    pub(crate) ramp: [RgbaF32; 4],
}
