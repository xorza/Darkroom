//! [`RingReference`]: the dark level a hot-pixel candidate is held to, from the photosites of its
//! color around it.

use common::Buffer2;

use crate::image::cfa::cfa_type::CfaType;
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
    dx: f64,
    dy: f64,
    value: f64,
}

/// `a + b·dx + c·dy`.
#[derive(Debug, Clone, Copy)]
struct Plane {
    a: f64,
    b: f64,
    c: f64,
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
                        dx: dx as f64,
                        dy: dy as f64,
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
        let mut level = Plane {
            a: median_of(
                &mut self.residuals,
                self.samples.iter().map(|sample| sample.value),
            ),
            b: 0.0,
            c: 0.0,
        };
        for _ in 0..2 {
            let cut = self.clip_cut(level);
            level = Plane::fit(&self.samples, |sample| {
                (sample.value - level.at(sample) - cut.centre).abs() <= cut.reach
            })?;
        }
        Some(level.a as f32)
    }

    /// The clip about `plane`: the median of the samples' residuals from it, and [`CLIP_SIGMAS`]
    /// of their σ from the MAD.
    fn clip_cut(&mut self, plane: Plane) -> Clip {
        self.residuals.clear();
        self.residuals.extend(
            self.samples
                .iter()
                .map(|sample| sample.value - plane.at(sample)),
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

impl Plane {
    /// The least-squares plane through the samples `keep` passes; `None` when they fix none.
    fn fit(samples: &[RingSample], keep: impl Fn(&RingSample) -> bool) -> Option<Self> {
        let mut n = 0.0;
        let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
        let (mut sz, mut sxz, mut syz) = (0.0, 0.0, 0.0);
        for sample in samples.iter().filter(|sample| keep(sample)) {
            let RingSample { dx, dy, value } = *sample;
            n += 1.0;
            sx += dx;
            sy += dy;
            sxx += dx * dx;
            syy += dy * dy;
            sxy += dx * dy;
            sz += value;
            sxz += dx * value;
            syz += dy * value;
        }
        // Cramer's rule on the normal equations [n sx sy; sx sxx sxy; sy sxy syy]·[a b c] = [sz sxz
        // syz].
        let det3 = |m: [[f64; 3]; 3]| {
            m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
        };
        let matrix = [[n, sx, sy], [sx, sxx, sxy], [sy, sxy, syy]];
        let det = det3(matrix);
        if det == 0.0 || det.is_nan() || n < 3.0 {
            return None;
        }
        let rhs = [sz, sxz, syz];
        let with_col = |col: usize| {
            let mut m = matrix;
            for (row, value) in rhs.iter().enumerate() {
                m[row][col] = *value;
            }
            det3(m) / det
        };
        Some(Self {
            a: with_col(0),
            b: with_col(1),
            c: with_col(2),
        })
    }

    fn at(self, sample: &RingSample) -> f64 {
        self.a + self.b * sample.dx + self.c * sample.dy
    }
}
