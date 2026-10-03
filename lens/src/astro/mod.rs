//! The `astro` domain — `lumos`-backed nodes (category `Astro`). Frames flow
//! on graph wires as the same [`Image`](crate::image::Image) the image nodes
//! pass: the astro nodes produce it planar, and an edge into an image node
//! repacks it once. See [`nodes`] for the node library.

mod config;
mod masters;
pub(crate) mod nodes;
