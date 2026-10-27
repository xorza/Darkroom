//! Reusable, domain-agnostic UI widgets composed from palantir
//! primitives, and the gesture and zoom machinery more than one surface
//! builds on. Unlike the `node`/`canvas` modules these hold no graph
//! knowledge — a caller maps the widget's returned event onto its own
//! intent.
pub(crate) mod chip;
pub(crate) mod edit_buffer;
pub(crate) mod format;
pub(crate) mod hover;
pub(crate) mod icon;
pub(crate) mod icon_button;
pub(crate) mod inline_rename;
pub(crate) mod pan_drag;
pub(crate) mod pill;
pub(crate) mod port_glyph;
pub(crate) mod support;
pub(crate) mod toggle;
pub(crate) mod zoom_range;
