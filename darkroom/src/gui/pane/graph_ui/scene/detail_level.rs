//! [`DetailLevel`]: how much of a node the canvas paints at a zoom.

use palantir::Visibility;

/// How much of a node the canvas paints. Below a legible size a node draws its title alone: as
/// Unreal's graph panel picks a level of detail from the zoom
/// ([SNodePanel](https://dev.epicgames.com/documentation/unreal-engine/API/Editor/GraphEditor/SNodePanel))
/// and a pin draws its low-detail look at the low ones
/// ([SGraphPin](https://dev.epicgames.com/documentation/unreal-engine/API/Editor/GraphEditor/SGraphPin)).
/// Everything else is laid out and not painted, so a node's size and its ports' centers, which
/// the canvas reads back from what it recorded, stay as they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetailLevel {
    Full,
    /// The title, at the caption size on screen, and the port glyphs alone.
    Title,
}

impl DetailLevel {
    /// The smallest body text, in screen pixels, a node draws its words at: under it, the
    /// face's x-height falls under 4 px.
    pub(crate) const LEGIBLE_PX: f32 = 8.0;

    /// `Full` while body text of `body_px` lands at [`Self::LEGIBLE_PX`] or over at `zoom`.
    pub(crate) fn at(zoom: f32, body_px: f32) -> Self {
        if body_px * zoom >= Self::LEGIBLE_PX {
            Self::Full
        } else {
            Self::Title
        }
    }

    /// How a node's words record at this level: laid out at both, painted at `Full` alone.
    pub(crate) const fn words(self) -> Visibility {
        match self {
            Self::Full => Visibility::Visible,
            Self::Title => Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use palantir::Visibility;

    use crate::gui::pane::graph_ui::scene::detail_level::DetailLevel;

    /// Body text of 14 px is legible down to 8 / 14 = 0.5714… zoom: 14 · 0.572 = 8.008 is
    /// `Full`, 14 · 0.571 = 7.994 is `Title`.
    #[test]
    fn a_zoom_that_lands_body_text_under_eight_pixels_draws_titles() {
        assert_eq!(DetailLevel::at(0.572, 14.0), DetailLevel::Full);
        assert_eq!(DetailLevel::at(0.571, 14.0), DetailLevel::Title);
        assert_eq!(DetailLevel::at(1.0, 14.0), DetailLevel::Full);
        assert_eq!(DetailLevel::Full.words(), Visibility::Visible);
        assert_eq!(DetailLevel::Title.words(), Visibility::Hidden);
    }
}
