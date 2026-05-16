//! Capability abstraction for the Session ↔ frontend boundary.
//!
//! Non-UI code (e.g. [`Session`]'s worker callbacks) sometimes needs
//! to signal the frontend — request a redraw when a background
//! computation finishes, or ask the app to close. [`UiHost`] exposes
//! exactly those two capabilities so the core can be driven by an
//! egui frontend, a terminal frontend, or a test harness without
//! dragging `egui::Context` (or any other frontend chrome) into
//! non-UI code.
//!
//! Per-frontend impls live next to their frontends:
//! `gui::ui_host::EguiUiHost`, `tui::ui_host::TuiUiHost`.
//!
//! [`Session`]: crate::session::Session

pub trait UiHost: Send + Sync + std::fmt::Debug {
    fn request_redraw(&self);
    fn close_app(&self);
}
