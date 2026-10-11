//! Bayer CFA demosaicing module.

pub(crate) mod bayer_image;
pub(crate) mod rcd;

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
