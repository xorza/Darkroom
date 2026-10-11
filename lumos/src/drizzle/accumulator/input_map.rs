//! [`InputMap`]: input pixels onto the output grid, and output regions back onto the input.

use glam::DVec2;

use crate::drizzle::accumulator::frame_source::Outline;
use crate::registration::transform::inverse_warp::InverseWarp;
use crate::registration::transform::{Transform, WarpTransform};

/// Output pixels between boundary samples when a SIP band's outline is taken back to the input.
///
/// A SIP edge bends away from the chord between two samples by at most `stride²/8·|∂²W|`, and a
/// fitted field's second derivative is far under 1e-2 per pixel, so at this stride the bend is
/// under 0.1 input pixel — inside the extra pixel the outline adds for it.
const SIP_BOUNDARY_STRIDE: usize = 8;

/// Input pixels onto the output grid, and output points back onto the input.
#[derive(Debug)]
pub(super) enum InputMap {
    /// A warp without SIP: one transform each way.
    Transform {
        to_output: Transform,
        to_input: Transform,
    },
    /// A warp with SIP — boxed, since it carries two copies of the polynomial and the source is
    /// built once per frame.
    Sip(Box<SipMap>),
}

/// [`InputMap::Sip`]: forward by the warp itself, backward by its Newton inverse, each with the
/// output grid's own map.
#[derive(Debug)]
pub(super) struct SipMap {
    warp: WarpTransform,
    inverse: InverseWarp,
    grid: Transform,
}

impl InputMap {
    /// `grid ∘ warp⁻¹`, the map from input pixels to output pixels.
    pub(super) fn new(warp: &WarpTransform, grid: Transform) -> Self {
        if warp.has_sip() {
            return Self::Sip(Box::new(SipMap {
                warp: warp.clone(),
                inverse: warp.inverse(),
                grid,
            }));
        }
        let to_output = grid.compose(&warp.transform.inverse());
        Self::Transform {
            to_output,
            to_input: to_output.inverse(),
        }
    }

    /// Where input point `p` lands on the output grid, or `None` when the SIP inverse does not
    /// converge there.
    #[inline]
    pub(super) fn position(&self, p: DVec2) -> Option<DVec2> {
        match self {
            Self::Transform { to_output, .. } => Some(to_output.apply(p)),
            Self::Sip(sip) => sip
                .inverse
                .position(p)
                .map(|position| sip.grid.apply(position)),
        }
    }

    /// The output rectangle `[left, right] × [low, high]` taken back onto the input, reaching
    /// `input_margin` beyond, or `None` when no outline bounds it and the whole frame has to be
    /// scanned.
    #[expect(
        clippy::cast_sign_loss,
        reason = "an output rectangle's width and height are non-negative, and an empty one saturates to 0"
    )]
    pub(super) fn outline(
        &self,
        low: f64,
        high: f64,
        left: f64,
        right: f64,
        input_margin: f64,
    ) -> Option<Outline> {
        match self {
            Self::Transform { to_input, .. } => {
                let corners = [
                    DVec2::new(left, low),
                    DVec2::new(right, low),
                    DVec2::new(right, high),
                    DVec2::new(left, high),
                ];
                // The corner hull bounds the interior only while the inverse's homogeneous divisor
                // keeps one sign across the rectangle. That divisor is affine in the output
                // coordinates, so a sign change between corners means the rectangle straddles the
                // vanishing line, where the mapped region is unbounded and four corners bound
                // nothing. Only a homography can do it — every other model divides by a constant 1.
                // On one side of it a homography maps the rectangle's edges to straight edges.
                let m = to_input.matrix();
                let divisor = |p: DVec2| m[6] * p.x + m[7] * p.y + m[8];
                let reference = divisor(corners[0]);
                if !corners
                    .iter()
                    .all(|&corner| divisor(corner) * reference > 0.0)
                {
                    return None;
                }
                Some(Outline {
                    vertices: corners.map(|corner| to_input.apply(corner)).to_vec(),
                    reach: input_margin,
                    bounds_columns: true,
                })
            }
            Self::Sip(sip) => {
                // A non-linear map bends straight edges, so the outline is sampled rather than its
                // corners taken, and one input pixel is added for the bend between samples — see
                // `SIP_BOUNDARY_STRIDE`. The image of a region's boundary bounds the image of the
                // region for any map without folds, which a converging inverse guarantees here.
                let from_grid = sip.grid.inverse();
                let back = |p: DVec2| sip.warp.apply(from_grid.apply(p));
                let along = |extent: f64| {
                    (0..=extent.ceil() as usize)
                        .step_by(SIP_BOUNDARY_STRIDE)
                        .map(move |offset| (offset as f64).min(extent))
                        .chain([extent])
                };
                let (width, height) = (right - left, high - low);
                let bottom = along(width).map(|dx| DVec2::new(left + dx, low));
                let right_edge = along(height).map(|dy| DVec2::new(right, low + dy));
                let top = along(width).map(|dx| DVec2::new(right - dx, high));
                let left_edge = along(height).map(|dy| DVec2::new(left, high - dy));
                Some(Outline {
                    vertices: bottom
                        .chain(right_edge)
                        .chain(top)
                        .chain(left_edge)
                        .map(back)
                        .collect(),
                    reach: input_margin + 1.0,
                    bounds_columns: false,
                })
            }
        }
    }
}
