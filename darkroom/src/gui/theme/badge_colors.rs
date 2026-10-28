//! [`BadgeColors`]: the inks of what a node's head says about itself.

use palantir::RgbaF32;

use crate::gui::theme::palette::Palette;

/// The marks a node's head carries beside its title. Two can show on one head, so
/// `theme::tests::badges_are_told_apart_from_each_other` holds them apart.
#[derive(Clone, Debug)]
pub(crate) struct BadgeColors {
    /// The persist-to-disk cache button.
    pub(crate) cache: RgbaF32,
    /// The kind mark: a read-only descriptor of what the node is (a sink, or recomputed every
    /// run), in the title's white, as a kind is not a status.
    pub(crate) kind: RgbaF32,
}

impl BadgeColors {
    pub(super) const fn from_palette(p: &Palette) -> Self {
        Self {
            cache: p.badge_cache,
            kind: p.badge_kind,
        }
    }
}
