//! The `astro` domain — `lumos`-backed nodes (category `astro`). Astronomical
//! frames flow on graph wires as the imaginarium-backed [`crate::image::Image`]
//! (RGB_F32), the same currency the imaginarium image nodes use, so the two
//! interoperate directly. See [`nodes`] for the node library.

mod config;
mod masters;
pub(crate) mod nodes;
