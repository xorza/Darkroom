//! Function-only facade over the colocated benchmark drivers.
//!
//! Every implementation lives in a `bench.rs` beside the code it measures; this
//! module is the single `bench`-gated surface the thin `benches/*.rs` targets
//! link against, so no production module has to be `pub` for a benchmark's
//! sake.

pub use crate::image::conversion::bench::bench as conversion;
pub use crate::ops::contrast_brightness::bench::bench as contrast_brightness;
pub use crate::ops::preview::bench::bench as preview;
pub use crate::ops::transform::bench::bench as transform;
