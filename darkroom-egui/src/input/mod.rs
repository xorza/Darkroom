//! Per-frame input sampling.
//!
//! Every user-intent read should go through [`InputSnapshot`] instead of
//! calling `ui.input(|i| …)` directly. Sampling once at frame start
//! avoids the race where separate closures observe different views of the
//! event stream (egui may deliver new events between closure invocations).

pub mod snapshot;

pub use snapshot::InputSnapshot;
