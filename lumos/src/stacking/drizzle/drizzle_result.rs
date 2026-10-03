//! [`DrizzleResult`]: what a drizzle produced, and what it could not place.

use crate::stacking::stack_product::StackProduct;

/// A drizzle's product, and the input pixels it could not place.
#[derive(Debug)]
pub struct DrizzleResult {
    pub product: StackProduct,
    /// Input pixels, over every frame, whose position a warp's SIP correction could not invert —
    /// see [`InverseWarp::apply`](crate::InverseWarp::apply). Each deposited nothing, so the output
    /// pixels it would have reached record that frame's absence in their coverage. Zero for frames
    /// registered without SIP.
    pub unconverged_points: usize,
}
