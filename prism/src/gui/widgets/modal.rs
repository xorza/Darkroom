//! Floating window backed by [`egui::Window`]. Title bar is the only
//! drag handle (body input is absorbed); bottom-right + edges resize.
//! Title bar gains a close `✕` when [`Modal::open`] is bound.
//!
//! Not strictly modal — there's no backdrop or input blocking against
//! widgets behind. Layer one at the call site if you need it.

use egui::{Sense, Vec2};

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
    /// window) flips the flag to false.
    pub fn open(mut self, open: &'a mut bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Minimum body size. Doubles as the default size on first open.
    pub fn min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = Some(min_size);
        self
    }

    /// Returns `Some(inner)` when the body ran this frame, `None` when
    /// the dialog is closed (or fully faded out).
    pub fn show<R>(self, gui: &mut Gui<'_>, body: impl FnOnce(&mut Gui<'_>) -> R) -> Option<R> {
        let ctx = gui.ui_raw().ctx().clone();
        let args = gui.child_args();

        let mut window = egui::Window::new(self.title)
            .id(self.id.id())
            .collapsible(false);
        if let Some(open) = self.open {
            window = window.open(open);
        }
        if let Some(min_size) = self.min_size {
            window = window.default_size(min_size).min_size(min_size);
        }

        let blocker_id = self.id.with("body_drag_block").id();
        window
            .show(&ctx, |ui| {
                args.enter(ui, |gui| {
                    // `egui::Window` forces `resizable: false` on its
                    // inner `Resize`, so the window's reported size is
                    // `last_content_size`. Claim the full available
                    // rect (= `Resize`'s `desired_size` each frame) so
                    // resize-by-edge sticks across frames.
                    let ui = gui.ui_raw();
                    ui.set_min_size(ui.available_size());
                    // Absorb body clicks/drags so the underlying Area's
                    // move-drag (registered earlier) only fires from
                    // the title bar.
                    let _ = ui.interact(
                        ui.available_rect_before_wrap(),
                        blocker_id,
                        Sense::click_and_drag(),
                    );
                    body(gui)
                })
            })
            .and_then(|r| r.inner)
    }
}
