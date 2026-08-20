//! [`TypeColors`]: the per-data-type ink roster.

use palantir::Color;

/// Data-type → wire/port-circle hue roster (consumed by
/// `gui::pane::graph::node::port_color`). Serialized as the theme's `[type_colors]`
/// table so a loaded theme file can restyle type hues like any other
/// swatch. `ramp` backs the open-ended `Custom`/`Enum` families —
/// keyed by `type_id`, so distinct custom types land on stable,
/// distinct colors; `image` is the fixed hue the lens image type owns.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TypeColors {
    pub(crate) boolean: Color,
    pub(crate) int: Color,
    pub(crate) float: Color,
    pub(crate) string: Color,
    pub(crate) path: Color,
    pub(crate) image: Color,
    pub(crate) ramp: [Color; 4],
}
