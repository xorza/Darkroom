//! [`ScreenshotAsk`]: how far one ask for a screenshot got, from the frame that asks to the one
//! its image lands in.

use palantir::{Rect, Screenshot, ScreenshotId, Ui};

/// One ask for an image of the window, made once, by the record pass of a frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum ScreenshotAsk {
    #[default]
    Unasked,
    /// The id the image lands under.
    Asked(ScreenshotId),
    /// The region held no pixel, so no image comes.
    Empty,
}

/// What a frame hands an ask.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Landed<'ui> {
    Shot(&'ui Screenshot),
    /// The ask got no image: its region held no pixel.
    Empty,
}

impl ScreenshotAsk {
    /// Ask `ui`'s frame for an image of `region`, the whole window for `None`, unless this ask
    /// went out already: a replayed record pass of the asking frame must not ask twice.
    pub(crate) fn ask(&mut self, ui: &mut Ui, region: Option<Rect>) {
        if matches!(self, Self::Unasked) {
            *self = ui
                .request_screenshot(region)
                .map_or(Self::Empty, Self::Asked);
        }
    }

    /// What this ask got, once it got it: its image in the frame `ui` it lands in, or none for a
    /// region that held no pixel.
    pub(crate) fn landed<'ui>(&self, ui: &'ui Ui) -> Option<Landed<'ui>> {
        match *self {
            Self::Unasked => None,
            Self::Asked(id) => ui.screenshot_taken(id).map(Landed::Shot),
            Self::Empty => Some(Landed::Empty),
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::UVec2;
    use palantir::Rect;
    use palantir::internals::ui_harness::UiHarness;

    use crate::gui::app::screenshot_ask::{Landed, ScreenshotAsk};

    /// An ask of a region the 64 × 48 window holds goes out once, and a second ask keeps the
    /// first; until its image lands, it got nothing. A region past the window holds no pixel: it
    /// lands as empty at once.
    #[test]
    fn an_ask_goes_out_once_and_an_empty_region_lands_at_once() {
        let mut h = UiHarness::new(UVec2::new(64, 48));
        let mut inside = ScreenshotAsk::default();
        let mut outside = ScreenshotAsk::default();
        h.frame(|ui| {
            inside.ask(ui, Some(Rect::new(0.0, 0.0, 10.0, 10.0)));
            let first = inside;
            inside.ask(ui, Some(Rect::new(20.0, 0.0, 10.0, 10.0)));
            assert!(matches!((first, inside), (ScreenshotAsk::Asked(a), ScreenshotAsk::Asked(b)) if a == b));
            assert!(inside.landed(ui).is_none(), "no image yet");
            outside.ask(ui, Some(Rect::new(500.0, 500.0, 10.0, 10.0)));
            assert!(matches!(outside.landed(ui), Some(Landed::Empty)));
        });
        assert!(ScreenshotAsk::default().landed(h.ui()).is_none(), "unasked");
    }
}
