//! Floating modal window backed by [`egui::Window`]. Title bar is the
//! only drag handle (body input is absorbed); the window auto-sizes
//! to its content (no user resize). Title bar gains a close `✕` when
//! [`Modal::open`] is bound.
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

use egui::{Align2, Order, Sense, Vec2, vec2};

use crate::common::StableId;
use crate::gui::Gui;

#[derive(Debug)]
#[must_use = "Modal does nothing until .show() is called"]
pub struct Modal<'a> {
    id: StableId,
    title: &'a str,
    open: Option<&'a mut bool>,
}

impl<'a> Modal<'a> {
    pub fn new(id: StableId, title: &'a str) -> Self {
        Self {
            id,
            title,
            open: None,
        }
    }

    /// Bind visibility to a caller-owned bool. Adds a close `✕` to the
    /// title bar; clicking it (or pressing Escape on the focused
    /// window) flips the flag to false.
    pub fn open(mut self, open: &'a mut bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Returns `Some(inner)` when the body ran this frame, `None` when
    /// the dialog is closed (or fully faded out).
    pub fn show<R>(self, gui: &mut Gui<'_>, body: impl FnOnce(&mut Gui<'_>) -> R) -> Option<R> {
        let ctx = gui.ui_raw().ctx().clone();
        let args = gui.view_params();
        let id = self.id;

        let visible = self.open.as_deref().copied().unwrap_or(true);
        if visible {
            gui.set_modal_layer(id);
            install_modal_chrome(&ctx, id);
        }

        // Stock `egui::Window` builds its frame from the global
        // `window_margin` (= `style.padding`, ~4 px), which leaves
        // dialog content visibly cramped against the chrome. Override
        // with `modal_padding` so modal bodies get the same breathing
        // room a real settings/about dialog expects.
        let frame = egui::Frame::window(&ctx.global_style())
            .inner_margin(egui::Margin::same(gui.style.modal_padding as i8));
        // Size to content with a finite, capped width.
        //
        // - `resizable(false)` makes the window's rendered size track
        //   `last_content_size` each frame (egui `Resize::show`,
        //   lines 327–329) — height auto-fits.
        // - `default_size = (420, 200)` gives `Gui::form_row` a finite
        //   `available_width` to allocate against (`auto_sized()`'s
        //   INFINITY width would inflate `min_rect.x` via
        //   `expand_to_include_rect` in egui's allocator).
        // - `max_width = default_size.x` caps the width so it can't
        //   keep growing toward the parent rect width: egui's
        //   `Resize::desired_size` only expands (never shrinks), and
        //   `form_row` reads `available_width = desired_size.x`, so
        //   without a cap each frame would feed back wider until it
        //   filled the panel.
        // - `min_size = 0` lets height shrink when rows disappear.
        // - `constrain_to(panel_rect)` clamps position+size to the
        //   central panel.
        // - No `vscroll` — its `min_scrolled_size` floor and outer
        //   bookkeeping add height even when content fits.
        let constrain_rect = gui.container_rect();
        let modal_width = 420.0;
        let mut window = egui::Window::new(self.title)
            .id(id.id())
            .collapsible(false)
            .resizable(false)
            .min_size(Vec2::ZERO)
            .max_width(modal_width)
            .default_size(vec2(modal_width, 200.0))
            .constrain_to(constrain_rect)
            .frame(frame);
        if let Some(open) = self.open {
            window = window.open(open);
        }

        window
            .show(&ctx, |ui| {
                args.enter(ui, |gui| {
                    let ui = gui.ui_raw();
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
