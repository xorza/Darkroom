//! Floating dialog backed by `egui::Window`. Title bar with close
//! button (when `.open(&mut bool)` is bound), resize handle, and
//! collapsible panel come from egui — this widget just bridges to
//! `Gui<'_>`.
//!
//! Not strictly modal: input still flows to widgets behind it. Add a
//! backdrop layer at the call site if you need that.

use egui::Vec2;

use crate::common::StableId;
use crate::gui::Gui;

#[derive(Debug)]
#[must_use = "Modal does nothing until .show() is called"]
pub struct Modal<'a> {
    id: StableId,
    title: &'a str,
    open: Option<&'a mut bool>,
    min_size: Option<Vec2>,
}

impl<'a> Modal<'a> {
    pub fn new(id: StableId, title: &'a str) -> Self {
        Self {
            id,
            title,
            open: None,
            min_size: None,
        }
    }

    /// Bind visibility to a caller-owned bool. Adds a close button to
    /// the title bar; clicking it (or pressing Escape on the focused
    /// window) flips the flag back to false. The body is skipped when
    /// the flag is already false.
    pub fn open(mut self, open: &'a mut bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Minimum body size. Doubles as the default size on first open.
    pub fn min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = Some(min_size);
        self
    }

    /// Returns `Some(inner)` when the dialog is open this frame, `None`
    /// when closed.
    pub fn show<R>(self, gui: &mut Gui<'_>, body: impl FnOnce(&mut Gui<'_>) -> R) -> Option<R> {
        if let Some(open) = &self.open
            && !**open
        {
            return None;
        }

        let ctx = gui.ui_raw().ctx().clone();
        let args = gui.child_args();

        let mut window = egui::Window::new(self.title)
            .id(self.id.id())
            .resizable(true)
            .collapsible(false);
        if let Some(min_size) = self.min_size {
            window = window.default_size(min_size).min_size(min_size);
        }

        // egui::Window forces `resizable: false` on its inner `Resize`,
        // so the window's reported size is `last_content_size`, not
        // `desired_size`. For an empty/sparse body that's whatever the
        // body claimed, ignoring drag-resize. Claim the full available
        // rect (which `Resize` sets to `desired_size` each frame) so
        // `last_content_size == desired_size` and resize sticks across
        // frames.
        let body_fill = move |gui: &mut Gui<'_>| {
            let avail = gui.ui_raw().available_size();
            gui.ui_raw().set_min_size(avail);
            body(gui)
        };

        let inner_resp = if let Some(open) = self.open {
            window.open(open).show(&ctx, |ui| args.enter(ui, body_fill))
        } else {
            window.show(&ctx, |ui| args.enter(ui, body_fill))
        };

        inner_resp.and_then(|r| r.inner)
    }
}
