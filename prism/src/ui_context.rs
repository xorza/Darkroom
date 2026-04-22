//! Capability token for the UI↔worker boundary.
//!
//! Non-UI code (e.g. [`Session`]'s worker callbacks) sometimes needs
//! to poke egui — request a redraw when a background computation
//! finishes, or send the viewport a close command. `UiContext` is a
//! cloneable handle to the egui `Context` that exposes only those
//! two capabilities; the full `egui::Context` is chrome that app
//! code shouldn't touch directly.
//!
//! Living in its own module (not `main_ui.rs` or `session.rs`) so
//! both sides of the boundary import it neutrally.
//!
//! [`Session`]: crate::session::Session

use eframe::egui;
use egui::ViewportCommand;

#[derive(Clone, Debug)]
pub struct UiContext {
    ctx: egui::Context,
}

impl UiContext {
    pub fn new(ctx: &egui::Context) -> Self {
        Self { ctx: ctx.clone() }
    }

    pub fn request_redraw(&self) {
        self.ctx.request_repaint();
    }

    pub fn close_app(&self) {
        self.ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}
