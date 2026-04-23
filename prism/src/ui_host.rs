//! Capability abstraction for the Session ↔ host boundary.
//!
//! Non-UI code (e.g. [`Session`]'s worker callbacks) sometimes needs
//! to signal the host — request a redraw when a background
//! computation finishes, or ask the app to close. [`UiHost`] exposes
//! exactly those two capabilities so the core can be driven by an
//! egui frontend, a headless runner, or a test harness without
//! dragging `egui::Context` into non-UI code.
//!
//! [`Session`]: crate::session::Session

use std::sync::Arc;

use eframe::egui;
use egui::ViewportCommand;

pub trait UiHost: Send + Sync + std::fmt::Debug {
    fn request_redraw(&self);
    fn close_app(&self);
}

pub type UiContext = Arc<dyn UiHost>;

#[derive(Debug)]
pub struct EguiUiHost {
    ctx: egui::Context,
}

impl EguiUiHost {
    pub fn new(ctx: &egui::Context) -> Self {
        Self { ctx: ctx.clone() }
    }
}

impl UiHost for EguiUiHost {
    fn request_redraw(&self) {
        self.ctx.request_repaint();
    }

    fn close_app(&self) {
        self.ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}
