//! X-Trans CFA demosaicing module.
//!
//! Provides demosaicing for Fujifilm X-Trans sensors which use a 6x6 CFA pattern
//! instead of the standard 2x2 Bayer pattern.
//!
//! The X-Trans pattern has ~55% green, ~22.5% red, and ~22.5% blue pixels arranged
//! so that every row and column contains all three colors.
//!
//! Uses the Markesteijn algorithm, one pass or three: directional interpolation with
//! homogeneity-based selection.

pub(crate) mod markesteijn;

#[cfg(test)]
mod tests;
