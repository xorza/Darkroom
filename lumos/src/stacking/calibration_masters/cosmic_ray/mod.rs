//! Single-frame cosmic-ray rejection via Laplacian edge detection (L.A.Cosmic, van Dokkum 2001).
//!
//! Cosmic rays and satellite/airplane streaks are sharp, single-frame events that stack-time
//! sigma/winsor rejection can't out-vote on short sequences. L.A.Cosmic flags them in a *single*
//! calibrated frame: a CR has sharper edges than a (PSF-broadened) star, so a Laplacian highlights
//! it, and a fine-structure test separates CRs from real point sources. Flagged pixels are
//! in-painted with the median of their unflagged neighbors, then the detect→replace loop repeats so
//! multi-pixel hits are fully removed.
//!
//! Runs on the calibrated, linear `CfaImage` before demosaic/registration (warping or demosaic
//! would smear a hit across pixels).
//!
//! Dispatches per CFA type: **Mono** = textbook subsampled L.A.Cosmic; **Bayer** = deinterleave the
//! four 2×2 phases and reuse the mono detector per dense same-color plane; **X-Trans** = same-color
//! stencils on the mosaic via `color_at` (no dense same-color sub-lattice exists there).
//!
//! **CFA caveat:** L.A.Cosmic assumes a PSF-sampled image, but a Bayer phase plane is half-resolution
//! — a tight star (FWHM ≲ 2–3 px in the mosaic) becomes ~1 px there, where the CR-vs-star
//! fine-structure test weakens. This per-frame rejection is therefore best for **short, un-dithered**
//! sequences; for dithered sets prefer dither + stack-time σ/winsor rejection, which out-votes CRs
//! without a per-frame discriminator. (`xtrans_removes_cosmic_ray...` / `bayer_tight_star...` tests
//! pin the tight-star behavior.)

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::math::statistics::{mad_f32_fast, mad_to_sigma, median_f32_mut};

/// `F` is floored to this (in normalized pixel units) so it stays non-negative where the object fine
/// structure is ~0 (i.e. at a CR).
const FINE_STRUCTURE_FLOOR: f32 = 1e-6;

/// Floor for the **noise-normalized** fine structure `F/noise` in the contrast test (in σ units).
/// Matches astroscrappy's `f.clip(min=0.01)` — bounds the `S'/(F/noise)` ratio where fine structure
/// is ~0 so a CR (F→0) doesn't divide by zero.
const FINE_STRUCTURE_SIGMA_FLOOR: f32 = 0.01;

/// Laplacian-edge cosmic-ray detection parameters. Defaults match ccdproc/astroscrappy.
#[derive(Debug, Clone)]
pub struct CosmicRayConfig {
    /// σ_lim: Laplacian-to-noise significance threshold (lower → more sensitive). Default 4.5.
    pub sigclip: f32,
    /// f_lim: minimum CR-to-fine-structure contrast separating CRs from PSF-broadened stars.
    /// Default 5.0.
    pub objlim: f32,
    /// Fraction of `sigclip` used when growing the mask onto a flagged CR's fainter wings. Default 0.3.
    pub sigfrac: f32,
    /// Maximum detect→replace iterations (multi-pixel CRs need several). Default 4.
    pub niter: usize,
    /// How per-pixel noise is estimated for the significance image.
    pub noise: NoiseEstimation,
}

impl Default for CosmicRayConfig {
    fn default() -> Self {
        Self {
            sigclip: 4.5,
            objlim: 5.0,
            sigfrac: 0.3,
            niter: 4,
            noise: NoiseEstimation::Empirical,
        }
    }
}

/// Per-pixel noise `N` for the significance image `S = L⁺/N` (the mono path adds a ½ for its ×2
/// subsample). Shared by all CFA paths.
#[derive(Debug, Clone)]
pub enum NoiseEstimation {
    /// Self-calibrating: a robust background σ (MAD) as the read-noise floor, scaled by the
    /// median-filtered signal for the Poisson term. Needs no camera parameters (default).
    ///
    /// This is a pragmatic approximation, **not** the canonical L.A.Cosmic noise model — ccdproc/
    /// astroscrappy always work in electrons (use [`NoiseEstimation::Parametric`] for that). It
    /// assumes a **sky-Poisson-dominated background** (the Poisson slope is anchored at the
    /// background, `σ_bg²/bg`), so on read-noise-dominated frames it over-estimates noise in bright
    /// regions and therefore slightly *under*-flags there. Chosen as the default because `gain`/
    /// `read_noise` are often unknown or unreliable for normalized data.
    Empirical,
    /// Exact Poisson + read noise `N_e = √(gain·I_ADU + read_noise²)`, converted from lumos's
    /// normalized `[0,1]` pixels via `full_scale` (`I_ADU = I_norm · full_scale`).
    Parametric {
        /// e⁻/ADU.
        gain: f32,
        /// Read noise, e⁻.
        read_noise: f32,
        /// ADU value that maps to normalized `1.0` (e.g. 4095 for a 12-bit sensor).
        full_scale: f32,
    },
}

/// Detect and in-paint cosmic rays in a single calibrated frame, in place, dispatching on its CFA
/// type (mono / Bayer / X-Trans). Returns the number of CR pixels corrected.
pub(crate) fn reject_cosmic_rays(image: &mut CfaImage, config: &CosmicRayConfig) -> usize {
    match &image.metadata.cfa_type {
        // Bayer is 2×2-periodic → four dense same-color planes; reuse the mono detector per plane.
        Some(CfaType::Bayer(_)) => reject_bayer(&mut image.data, config),
        // X-Trans has no dense same-color sub-lattice → same-color stencils on the mosaic.
        Some(c @ CfaType::XTrans(_)) => reject_xtrans(&mut image.data, c, config),
        // Mono (or an unlabeled frame): the dense Laplacian path.
        _ => reject_mono_buffer(&mut image.data, config),
    }
}

/// Monochrome L.A.Cosmic on one plane (also each deinterleaved Bayer plane). Subsample ×2 → clipped
/// Laplacian → resample → significance `S = L⁺/(2N)` → `S' = S − median₅(S)` → fine structure `F`
/// → flag → grow → in-paint → iterate. Returns the CR pixel count.
fn reject_mono_buffer(data: &mut Buffer2<f32>, config: &CosmicRayConfig) -> usize {
    let size = Size2us::new(data.width(), data.height());
    if size.width < 3 || size.height < 3 {
        return 0;
    }
    let mut mask = vec![false; size.pixel_count()];

    for _ in 0..config.niter {
        let pix = data.pixels();

        // L⁺: clipped Laplacian of the ×2-subsampled frame, averaged back to native resolution.
        let sub = subsample2(pix, size);
        let lplus = laplacian_plus(&sub, size);

        // Object fine structure F = median₃(I) − median₇(median₃(I)); large for real sources, ~0 at
        // a CR (median₃ already erased the spike).
        let m3 = median_window(pix, size, 1);
        let m37 = median_window(&m3, size, 3);
        let f: Vec<f32> = m3
            .iter()
            .zip(&m37)
            .map(|(&a, &b)| (a - b).max(FINE_STRUCTURE_FLOOR))
            .collect();

        // Significance S = L⁺/(2N), then S' = S − median₅(S) to strip smooth large-scale structure.
        let m5 = median_window(pix, size, 2);
        let noise = noise_map(pix, &m5, &config.noise);
        let s: Vec<f32> = lplus
            .iter()
            .zip(&noise)
            .map(|(&l, &nz)| l / (2.0 * nz))
            .collect();
        let s_med5 = median_window(&s, size, 2);
        let sprime: Vec<f32> = s.iter().zip(&s_med5).map(|(&a, &b)| a - b).collect();

        let flags = detect_and_grow(&sprime, &f, &noise, &mask, size, config);

        let mut newly = 0usize;
        for (m, &flag) in mask.iter_mut().zip(&flags) {
            if flag && !*m {
                *m = true;
                newly += 1;
            }
        }
        if newly == 0 {
            break;
        }
        replace_flagged(data, size, &mask);
    }

    mask.iter().filter(|&&m| m).count()
}

/// Block-replicate `data` to twice `size` on each axis (each pixel → a 2×2 block).
fn subsample2(data: &[f32], size: Size2us) -> Vec<f32> {
    let w2 = size.width * 2;
    let mut out = vec![0.0f32; w2 * size.height * 2];
    out.par_chunks_mut(w2).enumerate().for_each(|(y2, row)| {
        let y = y2 / 2;
        for (x2, o) in row.iter_mut().enumerate() {
            *o = data[size.index_of(Vec2us::new(x2 / 2, y))];
        }
    });
    out
}

/// Convolve `sub` (the ×2 image, i.e. twice `size` on each axis) with the Laplacian
/// `[[0,−1,0],[−1,4,−1],[0,−1,0]]`, clip negatives to 0 (keep only sharp positive peaks), then 2×2
/// block-average back down to `size`. Edge-clamped.
fn laplacian_plus(sub: &[f32], size: Size2us) -> Vec<f32> {
    let (w2, h2) = (size.width * 2, size.height * 2);
    let mut lap = vec![0.0f32; w2 * h2];
    lap.par_chunks_mut(w2).enumerate().for_each(|(y, row)| {
        let yu = y.saturating_sub(1);
        let yd = (y + 1).min(h2 - 1);
        for (x, o) in row.iter_mut().enumerate() {
            let xl = x.saturating_sub(1);
            let xr = (x + 1).min(w2 - 1);
            let c = sub[y * w2 + x];
            let v =
                4.0 * c - sub[yu * w2 + x] - sub[yd * w2 + x] - sub[y * w2 + xl] - sub[y * w2 + xr];
            *o = v.max(0.0);
        }
    });

    let mut lplus = vec![0.0f32; size.pixel_count()];
    lplus
        .par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, row)| {
            let (r0, r1) = (2 * y * w2, (2 * y + 1) * w2);
            for (x, o) in row.iter_mut().enumerate() {
                let (c0, c1) = (2 * x, 2 * x + 1);
                *o = 0.25 * (lap[r0 + c0] + lap[r0 + c1] + lap[r1 + c0] + lap[r1 + c1]);
            }
        });
    lplus
}

/// Median over a `(2r+1)²` window, edge-clamped. Scalar, row-parallel.
fn median_window(data: &[f32], size: Size2us, r: usize) -> Vec<f32> {
    let ri = r as isize;
    let (wi, hi) = (size.width as isize, size.height as isize);
    let mut out = vec![0.0f32; size.pixel_count()];
    out.par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, row)| {
            let mut buf: Vec<f32> = Vec::with_capacity((2 * r + 1) * (2 * r + 1));
            for (x, o) in row.iter_mut().enumerate() {
                buf.clear();
                for dy in -ri..=ri {
                    let yy = (y as isize + dy).clamp(0, hi - 1) as usize;
                    for dx in -ri..=ri {
                        let xx = (x as isize + dx).clamp(0, wi - 1) as usize;
                        buf.push(data[size.index_of(Vec2us::new(xx, yy))]);
                    }
                }
                *o = median_f32_mut(&mut buf);
            }
        });
    out
}

/// Per-pixel noise `N` from the median-filtered (CR-free) signal estimate `m5`.
fn noise_map(data: &[f32], m5: &[f32], noise: &NoiseEstimation) -> Vec<f32> {
    match *noise {
        NoiseEstimation::Empirical => {
            let mut tmp = data.to_vec();
            let bg = median_f32_mut(&mut tmp);
            let mut scratch = Vec::new();
            let sigma_bg = mad_to_sigma(mad_f32_fast(data, bg, &mut scratch)).max(1e-9);
            m5.iter()
                .map(|&s| empirical_noise(s, bg, sigma_bg))
                .collect()
        }
        NoiseEstimation::Parametric {
            gain,
            read_noise,
            full_scale,
        } => parametric_noise(m5, gain, read_noise, full_scale),
    }
}

/// Empirical per-pixel noise: a read-noise floor `σ` plus a sky-anchored Poisson term that rises as
/// `σ²·(signal−bg)/max(bg,σ)` above the background. Shared by the mono (whole-image `bg,σ`) and
/// X-Trans (per-color `bg,σ`) paths so the model can't drift between them.
#[inline]
fn empirical_noise(signal: f32, bg: f32, sigma: f32) -> f32 {
    let sigma2 = sigma * sigma;
    let slope = sigma2 / bg.max(sigma);
    (sigma2 + (signal - bg).max(0.0) * slope).sqrt()
}

/// Poisson + read noise per pixel from a CR-free signal estimate, in normalized units:
/// `N_e = √(gain·I_ADU + read_noise²)` mapped back through `full_scale`.
fn parametric_noise(signal: &[f32], gain: f32, read_noise: f32, full_scale: f32) -> Vec<f32> {
    let denom = gain * full_scale;
    signal
        .iter()
        .map(|&s| {
            let adu = s.max(0.0) * full_scale;
            ((gain * adu + read_noise * read_noise).sqrt() / denom).max(1e-9)
        })
        .collect()
}

/// Flag CRs: `S' > sigclip` **and** the fine-structure contrast `S' > objlim·(F/noise)`, then grow
/// onto neighbors clearing the lowered threshold `sigclip·sigfrac` and the same contrast test (a
/// flagged CR's fainter wings).
///
/// The contrast is van Dokkum's `L⁺/F > objlim` written in astroscrappy's noise-normalized form:
/// comparing the significance image `S'` against `objlim·(F/noise)` (rather than raw `L⁺` against
/// `objlim·F`) puts `F` in the same units as `S'`, so the `objlim` default carries the same
/// star-core protection as astroscrappy/ccdproc. (Raw `L⁺ > objlim·F` is ~2× more aggressive.)
fn detect_and_grow(
    significance: &[f32],
    f: &[f32],
    noise: &[f32],
    mask: &[bool],
    size: Size2us,
    cfg: &CosmicRayConfig,
) -> Vec<bool> {
    let passes_contrast = |i: usize, sig_thresh: f32| {
        let f_norm = (f[i] / noise[i]).max(FINE_STRUCTURE_SIGMA_FLOOR);
        significance[i] > sig_thresh && significance[i] > cfg.objlim * f_norm
    };
    let primary: Vec<bool> = (0..size.pixel_count())
        .map(|i| !mask[i] && passes_contrast(i, cfg.sigclip))
        .collect();

    let lowered = cfg.sigclip * cfg.sigfrac;
    let mut flags = primary.clone();
    for y in 0..size.height {
        for x in 0..size.width {
            if !primary[size.index_of(Vec2us::new(x, y))] {
                continue;
            }
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(size.height - 1);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(size.width - 1);
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    let j = size.index_of(Vec2us::new(nx, ny));
                    if !flags[j] && !mask[j] && passes_contrast(j, lowered) {
                        flags[j] = true;
                    }
                }
            }
        }
    }
    flags
}

/// Replace masked pixels with the median of their unmasked 5×5 neighbors (edge-clamped). Reads a
/// snapshot so replacements within one pass use pre-replacement values; fully-masked neighborhoods
/// (huge CRs) are left for the next iteration to shrink.
fn replace_flagged(data: &mut Buffer2<f32>, size: Size2us, mask: &[bool]) {
    let src = data.pixels().to_vec();
    let (wi, hi) = (size.width as isize, size.height as isize);
    data.pixels_mut()
        .par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, row)| {
            let mut buf: Vec<f32> = Vec::with_capacity(25);
            for (x, o) in row.iter_mut().enumerate() {
                if !mask[size.index_of(Vec2us::new(x, y))] {
                    continue;
                }
                buf.clear();
                for dy in -2..=2 {
                    let yy = (y as isize + dy).clamp(0, hi - 1) as usize;
                    for dx in -2..=2 {
                        let xx = (x as isize + dx).clamp(0, wi - 1) as usize;
                        let j = size.index_of(Vec2us::new(xx, yy));
                        if !mask[j] {
                            buf.push(src[j]);
                        }
                    }
                }
                if !buf.is_empty() {
                    *o = median_f32_mut(&mut buf);
                }
            }
        });
}

/// Bayer: the mosaic is 2×2-periodic, so pixels sharing a `(x%2, y%2)` phase are the same color and
/// form a dense plane. Deinterleave the four phases, run [`reject_mono_buffer`] on each (its dense
/// neighbors are same-color in the mosaic), and write the cleaned planes back. Pattern-independent —
/// phase alone determines color, so no `CfaPattern` is needed.
fn reject_bayer(data: &mut Buffer2<f32>, config: &CosmicRayConfig) -> usize {
    let w = data.width();
    let h = data.height();
    let mut total = 0;
    for b in 0..2 {
        for a in 0..2 {
            let pw = if a == 0 { w.div_ceil(2) } else { w / 2 };
            let ph = if b == 0 { h.div_ceil(2) } else { h / 2 };
            if pw < 3 || ph < 3 {
                continue;
            }
            let mut plane = vec![0.0f32; pw * ph];
            for j in 0..ph {
                for i in 0..pw {
                    plane[j * pw + i] = data[(j * 2 + b) * w + (i * 2 + a)];
                }
            }
            let mut buf = Buffer2::new(pw, ph, plane);
            total += reject_mono_buffer(&mut buf, config);
            let cleaned = buf.pixels();
            for j in 0..ph {
                for i in 0..pw {
                    data[(j * 2 + b) * w + (i * 2 + a)] = cleaned[j * pw + i];
                }
            }
        }
    }
    total
}

/// Radius (px) scanned for same-color neighbors — one X-Trans period (6×6) contains every color.
const XTRANS_RADIUS: i32 = 6;
/// Nearest same-color neighbors for the "fine" median; the coarse median uses all gathered.
const XTRANS_SMALL: usize = 8;
/// Cap on gathered same-color neighbors (the coarse median scale).
const XTRANS_LARGE: usize = 24;
/// Nearest unmasked same-color neighbors used to in-paint a flagged pixel.
const XTRANS_REPLACE: usize = 12;

/// Per-pixel detector inputs for the CFA path.
#[derive(Debug)]
struct XtransStructure {
    /// `max(0, v − median(nearest same-color))` — sharpness vs the same-color surroundings.
    lplus: Vec<f32>,
    /// Same-color fine structure `median_small − median_large` (large for sources, ~0 at a CR).
    f: Vec<f32>,
    /// CR-free signal estimate (the fine same-color median), for the noise model.
    signal: Vec<f32>,
}

/// X-Trans (and any non-Bayer CFA): no dense same-color sub-lattice, so detect on the mosaic with
/// same-color stencils gathered via [`CfaType::color_at`]. Median-based (robust to a CR inside the
/// stencil) and **without** the ×2 subsample — same-color sampling is already coarse and the
/// iteration handles multi-pixel hits. Significance is `S = L⁺/N`; no `S'` median-subtraction is
/// needed because `L⁺` (excess over the same-color median) is already a local high-pass.
fn reject_xtrans(data: &mut Buffer2<f32>, cfa: &CfaType, config: &CosmicRayConfig) -> usize {
    let size = Size2us::new(data.width(), data.height());
    if size.width < 7 || size.height < 7 {
        return 0;
    }
    let mut mask = vec![false; size.pixel_count()];

    for _ in 0..config.niter {
        let pix = data.pixels();
        let XtransStructure { lplus, f, signal } = xtrans_structure(pix, size, cfa, &mask);
        let noise = xtrans_noise(pix, size, cfa, &signal, &config.noise);
        let s: Vec<f32> = lplus.iter().zip(&noise).map(|(&l, &nz)| l / nz).collect();

        let flags = detect_and_grow(&s, &f, &noise, &mask, size, config);

        let mut newly = 0usize;
        for (m, &flag) in mask.iter_mut().zip(&flags) {
            if flag && !*m {
                *m = true;
                newly += 1;
            }
        }
        if newly == 0 {
            break;
        }
        xtrans_replace(data, cfa, &mask);
    }

    mask.iter().filter(|&&m| m).count()
}

/// Read-only context for same-color gathering: the plane data, its size, the CFA pattern, and the
/// current CR mask (gathered pixels exclude masked ones).
#[derive(Debug, Clone, Copy)]
struct CfaScene<'a> {
    pix: &'a [f32],
    size: Size2us,
    cfa: &'a CfaType,
    mask: &'a [bool],
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

/// Compute `L⁺`, `F`, and the signal estimate per pixel from same-color medians at two scales (one
/// gather per pixel: nearest-`XTRANS_LARGE`, with the nearest-`XTRANS_SMALL` subset).
fn xtrans_structure(pix: &[f32], size: Size2us, cfa: &CfaType, mask: &[bool]) -> XtransStructure {
    let (w, n) = (size.width, size.pixel_count());
    let mut lplus = vec![0.0f32; n];
    let mut f = vec![0.0f32; n];
    let mut signal = vec![0.0f32; n];
    let scene = CfaScene {
        pix,
        size,
        cfa,
        mask,
    };
    lplus
        .par_chunks_mut(w)
        .zip(f.par_chunks_mut(w))
        .zip(signal.par_chunks_mut(w))
        .enumerate()
        .for_each(|(y, ((lrow, frow), srow))| {
            let mut gathered: Vec<(i32, f32)> = Vec::with_capacity(64);
            let mut vals: Vec<f32> = Vec::with_capacity(XTRANS_LARGE);
            for x in 0..w {
                let v = pix[y * w + x];
                same_color_values(
                    &scene,
                    Vec2us::new(x, y),
                    XTRANS_RADIUS,
                    XTRANS_LARGE,
                    &mut gathered,
                );
                if gathered.is_empty() {
                    frow[x] = FINE_STRUCTURE_FLOOR;
                    srow[x] = v;
                    continue;
                }
                let small = gathered.len().min(XTRANS_SMALL);
                vals.clear();
                vals.extend(gathered[..small].iter().map(|&(_, val)| val));
                let med_small = median_f32_mut(&mut vals);
                vals.clear();
                vals.extend(gathered.iter().map(|&(_, val)| val));
                let med_large = median_f32_mut(&mut vals);
                lrow[x] = (v - med_small).max(0.0);
                frow[x] = (med_small - med_large).max(FINE_STRUCTURE_FLOOR);
                srow[x] = med_small;
            }
        });
    XtransStructure { lplus, f, signal }
}

/// Per-pixel noise for the CFA path. Empirical uses **per-color** background+σ (R/G/B sit at
/// different sky levels after flat-fielding, so a whole-mosaic MAD would be inflated); parametric is
/// color-independent (sensor gain), reusing the Poisson+read model on the same-color signal.
fn xtrans_noise(
    pix: &[f32],
    size: Size2us,
    cfa: &CfaType,
    signal: &[f32],
    noise: &NoiseEstimation,
) -> Vec<f32> {
    match *noise {
        NoiseEstimation::Empirical => {
            let mut by_color: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            for y in 0..size.height {
                for x in 0..size.width {
                    let c = (cfa.color_at(Vec2us::new(x, y)) as usize).min(2);
                    by_color[c].push(pix[y * size.width + x]);
                }
            }
            let mut scratch = Vec::new();
            let mut stats = [(0.0f32, 1e-9f32); 3];
            for (c, vals) in by_color.iter_mut().enumerate() {
                if vals.is_empty() {
                    continue;
                }
                let bg = median_f32_mut(vals);
                let sigma = mad_to_sigma(mad_f32_fast(vals, bg, &mut scratch)).max(1e-9);
                stats[c] = (bg, sigma);
            }
            (0..size.pixel_count())
                .map(|i| {
                    let p = size.point_of(i);
                    let c = (cfa.color_at(p) as usize).min(2);
                    let (bg, sigma) = stats[c];
                    empirical_noise(signal[i], bg, sigma)
                })
                .collect()
        }
        NoiseEstimation::Parametric {
            gain,
            read_noise,
            full_scale,
        } => parametric_noise(signal, gain, read_noise, full_scale),
    }
}

/// Replace masked pixels with the median of their nearest unmasked same-color neighbors.
fn xtrans_replace(data: &mut Buffer2<f32>, cfa: &CfaType, mask: &[bool]) {
    let size = Size2us::new(data.width(), data.height());
    let w = size.width;
    let src = data.pixels().to_vec();
    let scene = CfaScene {
        pix: &src,
        size,
        cfa,
        mask,
    };
    data.pixels_mut()
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
            let mut gathered: Vec<(i32, f32)> = Vec::with_capacity(32);
            let mut vals: Vec<f32> = Vec::new();
            for (x, o) in row.iter_mut().enumerate() {
                if !mask[y * w + x] {
                    continue;
                }
                same_color_values(
                    &scene,
                    Vec2us::new(x, y),
                    XTRANS_RADIUS,
                    XTRANS_REPLACE,
                    &mut gathered,
                );
                if gathered.is_empty() {
                    continue;
                }
                vals.clear();
                vals.extend(gathered.iter().map(|&(_, val)| val));
                *o = median_f32_mut(&mut vals);
            }
        });
}

#[cfg(test)]
mod tests;
