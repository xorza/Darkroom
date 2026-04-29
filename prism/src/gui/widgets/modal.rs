//! Modal dialog backed by `egui::Modal`. Dims the parent UI, captures
//! input, and closes on Escape or click outside.
//!
//! Wire `.open(&mut bool)` to drive visibility — the widget skips the
//! body when the flag is `false` and flips it back to `false` when the
//! user dismisses the modal. Callers don't need a separate `if open`
//! guard.

use egui::Vec2;

use crate::common::StableId;
use crate::gui::Gui;

#[derive(Debug)]
#[must_use = "Modal does nothing until .show() is called"]
pub struct Modal<'a> {
    id: StableId,
    open: Option<&'a mut bool>,
    min_size: Option<Vec2>,
}

impl<'a> Modal<'a> {
    pub fn new(id: StableId) -> Self {
        Self {
            id,
            open: None,
            min_size: None,
        }
    }

    /// Bind visibility to a caller-owned bool. The body runs only when
    /// `*open` is true; the widget resets it to `false` on dismissal
    /// (Escape / click outside).
    pub fn open(mut self, open: &'a mut bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = Some(min_size);
        self
    }

    /// Returns `Some(inner)` when the modal is open this frame, `None`
    /// when closed (matches [`crate::gui::widgets::PopupMenu::show`]).
    pub fn show<R>(self, gui: &mut Gui<'_>, body: impl FnOnce(&mut Gui<'_>) -> R) -> Option<R> {
        if let Some(open) = &self.open
            && !**open
        {
            return None;
        }

        let ctx = gui.ui_raw().ctx().clone();
        let args = gui.child_args();
        let min_size = self.min_size;
        let result = egui::Modal::new(self.id.id()).show(&ctx, |ui| {
            if let Some(size) = min_size {
                ui.set_min_size(size);
            }
            args.enter(ui, body)
        });

        if let Some(open) = self.open
            && result.should_close()
        {
            *open = false;
        }
        Some(result.inner)
    }
}
