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
    /// The title, sized with the canvas, and the port glyphs alone.
    Title,
}

impl DetailLevel {
    /// The smallest body text, in screen pixels, a node draws its words at: under it, the
    /// face's x-height falls under 3 px.
    pub(crate) const LEGIBLE_PX: f32 = 6.0;

    /// `Full` while body text of `body_px` lands at [`Self::LEGIBLE_PX`] or over at `zoom`.
    pub(crate) fn at(zoom: f32, body_px: f32) -> Self {
        if body_px * zoom >= Self::LEGIBLE_PX {
            Self::Full
        } else {
            Self::Title
        }
    }

    /// The canvas size of text that lands at `screen_px` on screen at the zoom where body text
    /// of `body_px` turns `Title`, so a title drawn at it reads that size as the level starts and
    /// shrinks with the canvas past it.
    pub(crate) fn title_px(screen_px: f32, body_px: f32) -> f32 {
        screen_px * body_px / Self::LEGIBLE_PX
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

    /// Body text of 14 px is legible down to 6 / 14 = 0.4285… zoom: 14 · 0.429 = 6.006 is
    /// `Full`, 14 · 0.428 = 5.992 is `Title`. A 12 px title at that zoom is 12 · 14 / 6 = 28 px
    /// on the canvas, 12 · 16 / 6 = 32 px over 16 px body text.
    #[test]
    fn a_zoom_that_lands_body_text_under_six_pixels_draws_titles() {
        assert_eq!(DetailLevel::at(0.429, 14.0), DetailLevel::Full);
        assert_eq!(DetailLevel::at(0.428, 14.0), DetailLevel::Title);
        assert_eq!(DetailLevel::at(1.0, 14.0), DetailLevel::Full);
        assert_eq!(DetailLevel::Full.words(), Visibility::Visible);
        assert_eq!(DetailLevel::Title.words(), Visibility::Hidden);
        assert_eq!(DetailLevel::title_px(12.0, 14.0), 28.0);
        assert_eq!(DetailLevel::title_px(12.0, 16.0), 32.0);
    }
}
