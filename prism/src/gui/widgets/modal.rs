//! Floating modal backed directly by [`egui::Area`] — no
//! [`egui::Window`], no [`egui::containers::resize::Resize`]. The Area
//! stores `state.size = content_ui.min_size()` each frame
//! (`area.rs:670`), so the modal tracks its content size both up and
//! down with no monotonic high-water mark. Title bar (with close `✕`)
//! is the only drag handle: `Area::movable(true)` registers a
//! whole-area drag, then a body-wide click+drag absorber consumes
//! input below the title bar so only title-bar clicks reach Area's
//! drag.
//!
//! Modal-layer (focus traversal blocked) + a transparent screen-sized
//! click+drag absorber backdrop come from [`install_modal_chrome`] —
//! same recipe as `egui::Modal`. No visual dim is painted; layer an
//! `egui::Area` at the call site if you want one.

use egui::{Align, Align2, Key, Layout, Modifiers, Order, Sense, Vec2};

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::widgets::{Button, Label, Separator};

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
    /// title bar; clicking it (or pressing Escape) flips the flag to
    /// false.
    pub fn open(mut self, open: &'a mut bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Returns `Some(inner)` when the body ran this frame, `None` when
    /// the dialog is closed.
    pub fn show<R>(self, gui: &mut Gui<'_>, body: impl FnOnce(&mut Gui<'_>) -> R) -> Option<R> {
        let visible = self.open.as_deref().copied().unwrap_or(true);
        if !visible {
            return None;
        }

        let ctx = gui.ui_raw().ctx().clone();
        let constrain_rect = gui.container_rect();
        let id = self.id;
        let title = self.title;

        gui.set_modal_layer(id);
        install_modal_chrome(&ctx, id);

        // Stock window margin (~4 px) leaves dialog content cramped;
        // bump to `modal_padding`.
        let frame = egui::Frame::window(&ctx.global_style())
            .inner_margin(egui::Margin::same(gui.style.modal_padding as i8));

        let args = gui.view_params();

        let mut close_clicked = false;

        let inner = egui::Area::new(id.id())
            .order(Order::Foreground)
            .movable(true)
            .pivot(Align2::CENTER_CENTER)
            .default_pos(constrain_rect.center())
            .constrain_to(constrain_rect)
            .show(&ctx, |ui| {
                frame
                    .show(ui, |ui| {
                        args.enter(ui, |gui| {
                            // Title bar: title on left, close × on
                            // right. `row_with_layout` is sizing-pass
                            // aware (claims width 0 during measure,
                            // available_width on the visible pass) so
                            // the modal's measured width is content-
                            // driven, not slack-driven.
                            gui.row_with_layout(Layout::right_to_left(Align::Center), |gui| {
                                // RTL → first child placed
                                // rightmost.
                                let close = Button::new(id.with("close")).text("✕").show(gui);
                                if close.clicked() {
                                    close_clicked = true;
                                }
                                Label::new(title).show(gui);
                            });

                            Separator::new().show(gui);

                            // Body input absorber — keeps Area's
                            // whole-rect drag from firing inside the
                            // body. Registered before body so body
                            // widgets, registered later, win for
                            // their own click points; the absorber
                            // only catches background clicks/drags.
                            let body_region = gui.ui_raw().available_rect_before_wrap();
                            let _ = gui.ui_raw().interact(
                                body_region,
                                id.with("body_drag_block").id(),
                                Sense::click_and_drag(),
                            );

                            body(gui)
                        })
                    })
                    .inner
            })
            .inner;

        let escape_pressed = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        if (close_clicked || escape_pressed)
            && let Some(open) = self.open
        {
            *open = false;
        }

        Some(inner)
    }
}

/// Transparent screen-sized click+drag absorber registered before the
/// modal (so it draws underneath). Keyed off the modal's id so
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
