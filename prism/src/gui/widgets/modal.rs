//! Floating modal window backed by [`egui::Window`]. Title bar is the
//! only drag handle (body input is absorbed); edges and corners
//! resize. Title bar gains a close `✕` when [`Modal::open`] is bound.
//!
//! Why `egui::Window` and not `egui::Modal`: we want drag-to-move and
//! edge-resize, which `egui::Modal` does not provide. The trade-off is
//! that we have to recreate the modal-layer + backdrop pieces by hand
//! (see `install_modal_chrome`) and live with one rough edge: during
//! egui::Window's built-in fade-out (after `*open` flips to false) the
//! chrome is gone but the window paints at reducing opacity for a few
//! frames — input briefly leaks through. egui::Window doesn't expose a
//! way to query in-progress opacity, so we accept it.
//!
//! Modal input blocking uses egui's modal-layer system together with a
//! transparent screen-sized click+drag absorber — same recipe as
//! [`egui::Modal`]. No visual dim is painted; layer an [`egui::Area`]
//! at the call site if you want one.

use egui::{Align2, Order, Sense, Vec2};

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

    /// Bind visibility to a caller-owned bool. Adds a close `✕` to the
    /// title bar; clicking it (or pressing Escape on the focused
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
        let id = self.id;

        let visible = self.open.as_deref().copied().unwrap_or(true);
        if visible {
            gui.set_modal_layer(id);
            install_modal_chrome(&ctx, id);
        }

        let mut window = egui::Window::new(self.title).id(id.id()).collapsible(false);
        if let Some(open) = self.open {
            window = window.open(open);
        }
        if let Some(min_size) = self.min_size {
            window = window.default_size(min_size).min_size(min_size);
        }

        window
            .show(&ctx, |ui| {
                args.enter(ui, |gui| {
                    let ui = gui.ui_raw();
                    // egui::Window forces `resizable: false` on its
                    // inner Resize, reporting `last_content_size` (not
                    // `desired_size`). Claim the full available rect
                    // (= desired_size each frame) so resize-by-edge
                    // sticks across frames.
                    ui.set_min_size(ui.available_size());
                    // Absorb body input so the underlying Area's
                    // move-drag (registered earlier) only fires from
                    // the title bar.
                    let _ = ui.interact(
                        ui.available_rect_before_wrap(),
                        id.with("body_drag_block").id(),
                        Sense::click_and_drag(),
                    );
                    body(gui)
                })
            })
            .and_then(|r| r.inner)
    }
}

/// Transparent screen-sized click+drag absorber registered before the
/// window (so it draws underneath). Keyed off the modal's id so
/// multiple modals coexist. Caller must also flip the modal-layer flag
/// via [`Gui::set_modal_layer`] for focus traversal to be blocked.
fn install_modal_chrome(ctx: &egui::Context, id: StableId) {
    let backdrop_id = id.with("backdrop").id();
    egui::Area::new(backdrop_id)
        .order(Order::Middle)
        .anchor(Align2::LEFT_TOP, Vec2::ZERO)
        .interactable(true)
        .show(ctx, |ui| {
            let _ = ui.interact(
                ui.ctx().content_rect(),
                backdrop_id.with("hit"),
                Sense::click_and_drag(),
            );
        });
}
