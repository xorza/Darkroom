//! Prelude: the full public surface in one glob import.
//!
//! Re-exports every item published at the crate root, so `use lumos::prelude::*`
//! reaches all of the loading, calibration, detection, registration, stacking,
//! and drizzle APIs. It mirrors `lib.rs` via a wildcard, so it cannot drift out
//! of sync as the surface grows.
//!
//! # Usage
//!
//! ```rust,ignore
//! use lumos::prelude::*;
//! ```

pub use crate::*;
