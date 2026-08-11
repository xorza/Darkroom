//! L.A.Cosmic where no dense same-colour sub-lattice exists.
//!
//! X-Trans has no 2×2 phase to deinterleave, so detection runs on the mosaic itself with
//! same-colour stencils gathered through [`CfaType::color_at`]. Median-based, so a cosmic ray
//! inside a stencil cannot drag its own reference, and without the ×2 subsample — same-colour
//! sampling is already coarse, and the iteration handles multi-pixel hits.

use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::io::image::cfa::CfaType;
use crate::math::size2us::Size2us;
use crate::math::statistics::{mad_f32_fast, mad_to_sigma, median_f32_mut, representational_floor};
use crate::math::vec2us::Vec2us;
use crate::stacking::calibration_masters::same_color::XTransOffsets;

use crate::stacking::calibration_masters::cosmic_ray::config::{CosmicRayConfig, NoiseEstimation};
use crate::stacking::calibration_masters::cosmic_ray::masks::CrMasks;
use crate::stacking::calibration_masters::cosmic_ray::mono::{
    empirical_noise, parametric_noise_into,
};

/// Radius (px) scanned for same-color neighbors — one X-Trans period (6×6) contains every color.
/// Nearest same-color neighbors for the "fine" median; the coarse median uses all gathered.
const XTRANS_SMALL: usize = 8;
/// Cap on gathered same-color neighbors (the coarse median scale).
const XTRANS_LARGE: usize = 24;
/// Nearest unmasked same-color neighbors used to in-paint a flagged pixel.
const XTRANS_REPLACE: usize = 12;

/// The CFA detector's per-pixel inputs and the scratch that builds them, allocated on the first
/// iteration and reused by every one after it — [`MonoDetector`](super::mono::MonoDetector)'s rule on the X-Trans path.
#[derive(Debug, Default)]
struct XtransScratch {
    /// `max(0, v − median(nearest same-color))` — sharpness vs the same-color surroundings — then
    /// the significance `S = L⁺/N` in place.
    lplus: Vec<f32>,
    /// Same-color fine structure `median_small − median_large` (large for sources, ~0 at a CR).
    f: Vec<f32>,
    /// CR-free signal estimate (the fine same-color median), for the noise model.
    signal: Vec<f32>,
    /// Per-pixel noise `N`.
    noise: Vec<f32>,
    /// The frame's pixels bucketed by color, for the per-color empirical background statistics.
    by_color: [Vec<f32>; 3],
    /// Frame-sized scratch: the deviations a per-color MAD consumes, then the read-only snapshot
    /// [`xtrans_replace`] gathers from — never both at once.
    frame: Vec<f32>,
}

/// The X-Trans detector: the pattern it reads colours from, its configuration, and the working
/// set it reuses across iterations.
///
/// Owning all three is what makes a run one object rather than three locals — the same shape the
/// mono and Bayer detectors take, so the dispatch reads alike for every pattern.
#[derive(Debug)]
pub(super) struct XtransDetector<'a> {
    cfa: &'a CfaType,
    config: &'a CosmicRayConfig,
    /// Same-colour neighbour geometry, built once per detector.
    ///
    /// Shared with the defect scan, which added it after finding that recomputing the neighbour set
    /// per pixel — a 13×13 `color_at` sweep plus a distance sort — dominated its own scan. This
    /// scan does the same walk at every pixel of every iteration, so it wants the table more.
    offsets: XTransOffsets,
    scratch: XtransScratch,
}

impl<'a> XtransDetector<'a> {
    pub(super) fn new(config: &'a CosmicRayConfig, cfa: &'a CfaType) -> Self {
        let CfaType::XTrans(pattern) = cfa else {
            panic!("XtransDetector requires an X-Trans pattern, got {cfa:?}");
        };
        Self {
            cfa,
            config,
            offsets: XTransOffsets::new(pattern),
            scratch: XtransScratch::default(),
        }
    }

    /// Detect and in-paint cosmic rays on the mosaic, in place, returning the CR pixel count.
    ///
    /// Median-based, so a ray inside a stencil cannot drag its own reference, and **without** the
    /// mono path's ×2 subsample — same-colour sampling is already coarse and the iteration handles
    /// multi-pixel hits. Significance is `S = L⁺/N` with no `S'` median subtraction, since `L⁺`
    /// (excess over the same-colour median) is already a local high-pass.
    pub(super) fn reject(&mut self, data: &mut [f32], size: Size2us) -> usize {
        debug_assert_eq!(data.len(), size.pixel_count());
        if size.width < 7 || size.height < 7 {
            return 0;
        }
        let mut masks = CrMasks::new(size);
        let scratch = &mut self.scratch;

        for _ in 0..self.config.niter {
            let scene = CfaScene {
                pix: data,
                size,
                cfa: self.cfa,
                mask: &masks.accumulated,
            };
            scratch.fill_structure(&scene, &self.offsets);
            scratch.fill_noise(&scene, &self.config.noise);
            // S = L⁺/N, elementwise over the same extent, so it runs down the L⁺ buffer.
            for (l, &nz) in scratch.lplus.iter_mut().zip(&scratch.noise) {
                *l /= nz;
            }

            if masks.detect_and_grow(&scratch.lplus, &scratch.f, &scratch.noise, self.config) == 0 {
                break;
            }
            xtrans_replace(
                data,
                size,
                self.cfa,
                &masks.accumulated,
                &self.offsets,
                &mut scratch.frame,
            );
        }

        masks.accumulated.count_ones()
    }
}

/// Read-only context for same-color gathering: the plane data, its size, the CFA pattern, and the
/// current CR mask (gathered pixels exclude masked ones).
#[derive(Debug, Clone, Copy)]
struct CfaScene<'a> {
    pix: &'a [f32],
    size: Size2us,
    cfa: &'a CfaType,
    mask: &'a BitBuffer2,
}

impl XtransScratch {
    /// Compute `L⁺`, `F`, and the signal estimate per pixel from same-color medians at two scales
    /// (one gather per pixel: nearest-`XTRANS_LARGE`, with the nearest-`XTRANS_SMALL` subset).
    fn fill_structure(&mut self, scene: &CfaScene, offsets: &XTransOffsets) {
        let (w, n) = (scene.size.width, scene.size.pixel_count());
        // Every element is written below, so only the length matters.
        self.lplus.resize(n, 0.0);
        self.f.resize(n, 0.0);
        self.signal.resize(n, 0.0);
        self.lplus
            .par_chunks_mut(w)
            .zip(self.f.par_chunks_mut(w))
            .zip(self.signal.par_chunks_mut(w))
            .enumerate()
            .for_each_init(
                || Vec::<f32>::with_capacity(XTRANS_LARGE),
                |gathered, (y, ((lrow, frow), srow))| {
                    for x in 0..w {
                        let v = scene.pix[y * w + x];
                        offsets.gather(
                            scene.pix,
                            scene.size,
                            Vec2us::new(x, y),
                            scene.mask,
                            XTRANS_LARGE,
                            gathered,
                        );
                        if gathered.is_empty() {
                            frow[x] = 0.0;
                            srow[x] = v;
                            continue;
                        }
                        // Nearest-first, so the two scales are prefixes of one gather. The coarse
                        // median reorders `gathered`, so the fine one is taken first.
                        let small = gathered.len().min(XTRANS_SMALL);
                        let med_small = median_f32_mut(&mut gathered[..small]);
                        let med_large = median_f32_mut(gathered);
                        lrow[x] = (v - med_small).max(0.0);
                        // Non-negative only — see the mono detector: the σ-unit floor downstream
                        // is what guards the contrast ratio, at any sample scale.
                        frow[x] = (med_small - med_large).max(0.0);
                        srow[x] = med_small;
                    }
                },
            );
    }

    /// Per-pixel noise for the CFA path, from the signal estimate [`Self::fill_structure`] left.
    /// Empirical uses **per-color** background+σ (R/G/B sit at different sky levels after
    /// flat-fielding, so a whole-mosaic MAD would be inflated); parametric is color-independent
    /// (sensor gain), reusing the Poisson+read model on the same-color signal.
    fn fill_noise(&mut self, scene: &CfaScene, noise: &NoiseEstimation) {
        let Self {
            signal,
            noise: out,
            by_color,
            frame,
            ..
        } = self;
        let size = scene.size;
        match *noise {
            NoiseEstimation::Empirical => {
                for vals in by_color.iter_mut() {
                    vals.clear();
                }
                for y in 0..size.height {
                    for x in 0..size.width {
                        let c = (scene.cfa.color_at(Vec2us::new(x, y)) as usize).min(2);
                        by_color[c].push(scene.pix[y * size.width + x]);
                    }
                }
                // Both the seed for a colour with no samples and the fallback for a flat one come
                // from the frame's own magnitude, so a scale below any fixed constant — which is
                // where a 32-bit FITS lands — still yields a usable, strictly positive σ.
                let frame_floor = representational_floor(scene.pix);
                let mut stats = [(0.0f32, frame_floor); 3];
                for (c, vals) in by_color.iter_mut().enumerate() {
                    if vals.is_empty() {
                        continue;
                    }
                    let bg = median_f32_mut(vals);
                    let sigma = mad_to_sigma(mad_f32_fast(vals, bg, frame)).max(frame_floor);
                    stats[c] = (bg, sigma);
                }
                out.clear();
                out.extend((0..size.pixel_count()).map(|i| {
                    let p = size.point_of(i);
                    let c = (scene.cfa.color_at(p) as usize).min(2);
                    let (bg, sigma) = stats[c];
                    empirical_noise(signal[i], bg, sigma)
                }));
            }
            NoiseEstimation::Parametric {
                gain,
                read_noise,
                full_scale,
            } => parametric_noise_into(signal, gain, read_noise, full_scale, out),
        }
    }
}

/// Replace masked pixels with the median of their nearest unmasked same-color neighbors. Gathers
/// from a snapshot in the caller's `snapshot` buffer, for the reason [`replace_flagged`](super::mono::replace_flagged) gives.
fn xtrans_replace(
    data: &mut [f32],
    size: Size2us,
    cfa: &CfaType,
    mask: &BitBuffer2,
    offsets: &XTransOffsets,
    snapshot: &mut Vec<f32>,
) {
    let w = size.width;
    snapshot.clear();
    snapshot.extend_from_slice(data);
    let scene = CfaScene {
        pix: snapshot,
        size,
        cfa,
        mask,
    };
    data.par_chunks_mut(w).enumerate().for_each_init(
        || Vec::<f32>::with_capacity(XTRANS_REPLACE),
        |gathered, (y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                if !mask[y * w + x] {
                    continue;
                }
                offsets.gather(
                    scene.pix,
                    scene.size,
                    Vec2us::new(x, y),
                    mask,
                    XTRANS_REPLACE,
                    gathered,
                );
                if gathered.is_empty() {
                    continue;
                }
                *o = median_f32_mut(gathered);
            }
        },
    );
}
