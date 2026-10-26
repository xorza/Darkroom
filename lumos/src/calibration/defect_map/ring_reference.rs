//! [`RingReference`]: the dark level a hot-pixel candidate is held to, from the photosites of its
//! color around it.

use common::Buffer2;

use crate::image::cfa::cfa_type::CfaType;
use crate::math::plane_moments::{Plane, PlaneMoments};
use crate::math::statistics;
use crate::math::statistics::median_mad::MedianMad;
use common::{Size2us, Vec2us};

/// The ring's inner reach, in pixels: a same-color cluster of up to this many pixels across keeps
/// most of itself out of its members' rings, and what it leaves in, the clip takes out.
const RING_INNER: usize = 6;
/// The ring's outer reach, in pixels: close enough that smooth structure is a plane across it to
/// well under the noise — glow curving by `f''` departs from its tangent plane by `f''·8²/2`.
const RING_OUTER: usize = 8;
/// The fewest photosites a ring needs for a plane and a clip; a frame too small to give a ring
/// keeps the mesh's verdict.
const MIN_RING_SAMPLES: usize = 12;
/// The clip, in σ of the residuals about the current fit: hot pixels in the ring are far past it.
const CLIP_SIGMAS: f64 = 3.0;

/// The level at a pixel from a robust plane through the photosites of its color 6 to 8 pixels
/// around it, in the Chebyshev metric.
///
/// The broad tile mesh is the screen, and this the confirmation: a pixel the mesh calls hot is hot
/// only if it also stands above this plane. Smooth structure the mesh cannot follow — amp glow
/// curving faster than its tiles, or rising past the outer tile centres — is a plane at the ring's
/// scale, which the fit takes in, also at a frame edge where the ring is one-sided and a median
/// would read the level beside the pixel instead of at it. A defect is a point or a compact
/// cluster, which the ring passes over. The ring's median level clips the samples first, then a
/// least-squares plane through those it keeps clips them again, and the plane through what is
/// left is the level: hot pixels in the ring, a third of it on one side, do not tilt it.
#[derive(Debug, Default)]
pub(crate) struct RingReference {
    samples: Vec<RingSample>,
    residuals: Vec<f64>,
}

/// The frame a ring is read from: its size, and its sample at each flat index.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RingFrame<F> {
    pub(crate) size: Size2us,
    pub(crate) value: F,
}

impl RingFrame<()> {
    /// `data` as it holds its samples.
    pub(crate) fn of(data: &Buffer2<f32>) -> RingFrame<impl Fn(usize) -> f64 + Copy + '_> {
        RingFrame {
            size: data.size(),
            value: |idx: usize| f64::from(data[idx]),
        }
    }
}

/// One photosite of the ring: its offset from the pixel, and its value.
#[derive(Debug, Clone, Copy)]
struct RingSample {
    offset: [i32; 2],
    value: f64,
}

impl RingReference {
    /// The plane's level at `point` of `frame`, of its color under `cfa`; `None` when the frame
    /// gives the ring too few photosites, or they fix no plane.
    pub(crate) fn at(
        &mut self,
        frame: RingFrame<impl Fn(usize) -> f64>,
        cfa: &CfaType,
        point: Vec2us,
    ) -> Option<f32> {
        let RingFrame { size, value } = frame;
        let color = cfa.slot_at(point);
        self.samples.clear();
        let reach = RING_OUTER as isize;
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                if dx.unsigned_abs().max(dy.unsigned_abs()) < RING_INNER {
                    continue;
                }
                let (Some(x), Some(y)) = (
                    point.x.checked_add_signed(dx),
                    point.y.checked_add_signed(dy),
                ) else {
                    continue;
                };
                let at = Vec2us::new(x, y);
                if x < size.width && y < size.height && cfa.slot_at(at) == color {
                    self.samples.push(RingSample {
                        offset: [dx, dy].map(|d| i32::try_from(d).expect("a ring offset is small")),
                        value: value(size.idx_of(at)),
                    });
                }
            }
        }
        if self.samples.len() < MIN_RING_SAMPLES {
            return None;
        }
        // Robust from the start: the ring's median level clips what a third of the ring could tilt
        // a least-squares plane toward, and the plane through what is left clips again at the
        // noise about itself.
        let mut plane = Plane {
            level: median_of(
                &mut self.residuals,
                self.samples.iter().map(|sample| sample.value),
            ),
            slope: [0.0; 2],
        };
        for _ in 0..2 {
            let cut = self.clip_cut(plane);
            let mut moments = PlaneMoments::default();
            for sample in &self.samples {
                if (sample.value - plane.at(sample.offset) - cut.centre).abs() <= cut.reach {
                    moments.add(sample.offset, sample.value);
                }
            }
            plane = moments.plane()?;
        }
        Some(plane.level as f32)
    }

    /// The clip about `plane`: the median of the samples' residuals from it, and [`CLIP_SIGMAS`]
    /// of their σ from the MAD.
    fn clip_cut(&mut self, plane: Plane) -> Clip {
        self.residuals.clear();
        self.residuals.extend(
            self.samples
                .iter()
                .map(|sample| sample.value - plane.at(sample.offset)),
        );
        let residual = MedianMad::of_mut(&mut self.residuals);
        Clip {
            centre: residual.median,
            reach: CLIP_SIGMAS * residual.sigma(),
        }
    }
}

/// Which residuals a clip keeps: those within `reach` of `centre`.
#[derive(Debug, Clone, Copy)]
struct Clip {
    centre: f64,
    reach: f64,
}

/// The median of `values`, through `scratch`.
fn median_of(scratch: &mut Vec<f64>, values: impl Iterator<Item = f64>) -> f64 {
    scratch.clear();
    scratch.extend(values);
    statistics::median_mut(scratch)
}
