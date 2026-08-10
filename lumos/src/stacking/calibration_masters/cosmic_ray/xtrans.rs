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
use crate::math::statistics::{mad_f32_fast, mad_to_sigma, median_f32_mut};
use crate::math::vec2us::Vec2us;

use crate::stacking::calibration_masters::cosmic_ray::FINE_STRUCTURE_FLOOR;
use crate::stacking::calibration_masters::cosmic_ray::config::{CosmicRayConfig, NoiseEstimation};
use crate::stacking::calibration_masters::cosmic_ray::masks::CrMasks;
use crate::stacking::calibration_masters::cosmic_ray::mono::{
    empirical_noise, parametric_noise_into,
};

/// Radius (px) scanned for same-color neighbors — one X-Trans period (6×6) contains every color.
const XTRANS_RADIUS: i32 = 6;
/// Nearest same-color neighbors for the "fine" median; the coarse median uses all gathered.
const XTRANS_SMALL: usize = 8;
/// Cap on gathered same-color neighbors (the coarse median scale).
const XTRANS_LARGE: usize = 24;
/// Nearest unmasked same-color neighbors used to in-paint a flagged pixel.
const XTRANS_REPLACE: usize = 12;

/// The CFA detector's per-pixel inputs and the scratch that builds them, allocated on the first
/// iteration and reused by every one after it — [`MonoScratch`](super::mono::MonoScratch)'s rule on the X-Trans path.
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

/// X-Trans (and any non-Bayer CFA): no dense same-color sub-lattice, so detect on the mosaic with
/// same-color stencils gathered via [`CfaType::color_at`]. Median-based (robust to a CR inside the
/// stencil) and **without** the ×2 subsample — same-color sampling is already coarse and the
/// iteration handles multi-pixel hits. Significance is `S = L⁺/N`; no `S'` median-subtraction is
/// needed because `L⁺` (excess over the same-color median) is already a local high-pass.
pub(super) fn reject_xtrans(
    data: &mut [f32],
    size: Size2us,
    cfa: &CfaType,
    config: &CosmicRayConfig,
) -> usize {
    debug_assert_eq!(data.len(), size.pixel_count());
    if size.width < 7 || size.height < 7 {
        return 0;
    }
    let mut masks = CrMasks::new(size);
    let mut scratch = XtransScratch::default();

    for _ in 0..config.niter {
        let scene = CfaScene {
            pix: data,
            size,
            cfa,
            mask: &masks.accumulated,
        };
        scratch.fill_structure(&scene);
        scratch.fill_noise(&scene, &config.noise);
        // S = L⁺/N, elementwise over the same extent, so it runs down the L⁺ buffer.
        for (l, &nz) in scratch.lplus.iter_mut().zip(&scratch.noise) {
            *l /= nz;
        }

        if masks.detect_and_grow(&scratch.lplus, &scratch.f, &scratch.noise, config) == 0 {
            break;
        }
        xtrans_replace(data, size, cfa, &masks.accumulated, &mut scratch.frame);
    }

    masks.accumulated.count_ones()
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

/// Gather same-color (`color_at`) neighbor `(manhattan_dist, value)` around `pos` within Chebyshev
/// `radius`, nearest-first, capped at `max`, skipping masked pixels. `out` is cleared and reused.
fn same_color_values(
    scene: &CfaScene,
    pos: Vec2us,
    radius: i32,
    max: usize,
    out: &mut Vec<(i32, f32)>,
) {
    out.clear();
    let my = scene.cfa.color_at(pos);
    let w = scene.size.width;
    let (wi, hi) = (scene.size.width as i32, scene.size.height as i32);
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = pos.x as i32 + dx;
            let ny = pos.y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= wi || ny >= hi {
                continue;
            }
            let (nxu, nyu) = (nx as usize, ny as usize);
            if scene.mask[nyu * w + nxu] {
                continue;
            }
            if scene.cfa.color_at(Vec2us::new(nxu, nyu)) == my {
                out.push((dx.abs() + dy.abs(), scene.pix[nyu * w + nxu]));
            }
        }
    }
    out.sort_unstable_by_key(|&(d, _)| d);
    out.truncate(max);
}

impl XtransScratch {
    /// Compute `L⁺`, `F`, and the signal estimate per pixel from same-color medians at two scales
    /// (one gather per pixel: nearest-`XTRANS_LARGE`, with the nearest-`XTRANS_SMALL` subset).
    fn fill_structure(&mut self, scene: &CfaScene) {
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
                || {
                    (
                        Vec::<(i32, f32)>::with_capacity(64),
                        Vec::<f32>::with_capacity(XTRANS_LARGE),
                    )
                },
                |(gathered, vals), (y, ((lrow, frow), srow))| {
                    for x in 0..w {
                        let v = scene.pix[y * w + x];
                        same_color_values(
                            scene,
                            Vec2us::new(x, y),
                            XTRANS_RADIUS,
                            XTRANS_LARGE,
                            gathered,
                        );
                        if gathered.is_empty() {
                            frow[x] = FINE_STRUCTURE_FLOOR;
                            srow[x] = v;
                            continue;
                        }
                        let small = gathered.len().min(XTRANS_SMALL);
                        vals.clear();
                        vals.extend(gathered[..small].iter().map(|&(_, val)| val));
                        let med_small = median_f32_mut(vals);
                        vals.clear();
                        vals.extend(gathered.iter().map(|&(_, val)| val));
                        let med_large = median_f32_mut(vals);
                        lrow[x] = (v - med_small).max(0.0);
                        frow[x] = (med_small - med_large).max(FINE_STRUCTURE_FLOOR);
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
                let mut stats = [(0.0f32, 1e-9f32); 3];
                for (c, vals) in by_color.iter_mut().enumerate() {
                    if vals.is_empty() {
                        continue;
                    }
                    let bg = median_f32_mut(vals);
                    let sigma = mad_to_sigma(mad_f32_fast(vals, bg, frame)).max(1e-9);
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
        || {
            (
                Vec::<(i32, f32)>::with_capacity(32),
                Vec::<f32>::with_capacity(XTRANS_REPLACE),
            )
        },
        |(gathered, vals), (y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                if !mask[y * w + x] {
                    continue;
                }
                same_color_values(
                    &scene,
                    Vec2us::new(x, y),
                    XTRANS_RADIUS,
                    XTRANS_REPLACE,
                    gathered,
                );
                if gathered.is_empty() {
                    continue;
                }
                vals.clear();
                vals.extend(gathered.iter().map(|&(_, val)| val));
                *o = median_f32_mut(vals);
            }
        },
    );
}
