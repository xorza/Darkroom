//! [`HoverColor`]: a resting colour and the one it brightens to.

use palantir::Color;

/// Two-state colour pack for chrome that lifts under the pointer —
/// the colour-granularity peer of palantir's `StatefulLook`: the pair
/// is structural (a hover variant can't exist without its rest), and
/// state → colour goes through one `pick`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct HoverColor {
    pub(crate) rest: Color,
    pub(crate) hover: Color,
}

impl HoverColor {
    #[inline]
    pub(crate) fn pick(&self, hovered: bool) -> Color {
        if hovered { self.hover } else { self.rest }
    }
}
