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
//! would smear a hit across pixels). See `docs/pipeline/02-calibration.md §4.2` and
//! `docs/pipeline/cosmic-ray-rejection-plan.md`.
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

use crate::math::vec2us::Vec2us;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::astro_image::cfa::{CfaImage, CfaType};
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
    let w = data.width();
    let h = data.height();
    if w < 3 || h < 3 {
        return 0;
    }
    let mut mask = vec![false; w * h];

    for _ in 0..config.niter {
        let pix = data.pixels();

        // L⁺: clipped Laplacian of the ×2-subsampled frame, averaged back to native resolution.
        let sub = subsample2(pix, w, h);
        let lplus = laplacian_plus(&sub, w * 2, h * 2, w, h);

        // Object fine structure F = median₃(I) − median₇(median₃(I)); large for real sources, ~0 at
        // a CR (median₃ already erased the spike).
        let m3 = median_window(pix, w, h, 1);
        let m37 = median_window(&m3, w, h, 3);
        let f: Vec<f32> = m3
            .iter()
            .zip(&m37)
            .map(|(&a, &b)| (a - b).max(FINE_STRUCTURE_FLOOR))
            .collect();

        // Significance S = L⁺/(2N), then S' = S − median₅(S) to strip smooth large-scale structure.
        let m5 = median_window(pix, w, h, 2);
        let noise = noise_map(pix, &m5, &config.noise);
        let s: Vec<f32> = lplus
            .iter()
            .zip(&noise)
            .map(|(&l, &nz)| l / (2.0 * nz))
            .collect();
        let s_med5 = median_window(&s, w, h, 2);
        let sprime: Vec<f32> = s.iter().zip(&s_med5).map(|(&a, &b)| a - b).collect();

        let flags = detect_and_grow(&sprime, &f, &noise, &mask, w, h, config);

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
        replace_flagged(data, w, h, &mask);
    }

    mask.iter().filter(|&&m| m).count()
}

/// Block-replicate `data` to `2w × 2h` (each pixel → a 2×2 block).
fn subsample2(data: &[f32], w: usize, h: usize) -> Vec<f32> {
    let w2 = w * 2;
    let mut out = vec![0.0f32; w2 * h * 2];
    out.par_chunks_mut(w2).enumerate().for_each(|(y2, row)| {
        let y = y2 / 2;
        for (x2, o) in row.iter_mut().enumerate() {
            *o = data[y * w + x2 / 2];
        }
    });
    out
}

/// Convolve `sub` (the ×2 image) with the Laplacian `[[0,−1,0],[−1,4,−1],[0,−1,0]]`, clip negatives
/// to 0 (keep only sharp positive peaks), then 2×2 block-average back to `w × h`. Edge-clamped.
fn laplacian_plus(sub: &[f32], w2: usize, h2: usize, w: usize, h: usize) -> Vec<f32> {
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

    let mut lplus = vec![0.0f32; w * h];
    lplus.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (r0, r1) = (2 * y * w2, (2 * y + 1) * w2);
        for (x, o) in row.iter_mut().enumerate() {
            let (c0, c1) = (2 * x, 2 * x + 1);
            *o = 0.25 * (lap[r0 + c0] + lap[r0 + c1] + lap[r1 + c0] + lap[r1 + c1]);
        }
    });
    lplus
}

/// Median over a `(2r+1)²` window, edge-clamped. Scalar, row-parallel.
fn median_window(data: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let ri = r as isize;
    let (wi, hi) = (w as isize, h as isize);
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let mut buf: Vec<f32> = Vec::with_capacity((2 * r + 1) * (2 * r + 1));
        for (x, o) in row.iter_mut().enumerate() {
            buf.clear();
            for dy in -ri..=ri {
                let yy = (y as isize + dy).clamp(0, hi - 1) as usize;
                for dx in -ri..=ri {
                    let xx = (x as isize + dx).clamp(0, wi - 1) as usize;
                    buf.push(data[yy * w + xx]);
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
    w: usize,
    h: usize,
    cfg: &CosmicRayConfig,
) -> Vec<bool> {
    let passes_contrast = |i: usize, sig_thresh: f32| {
        let f_norm = (f[i] / noise[i]).max(FINE_STRUCTURE_SIGMA_FLOOR);
        significance[i] > sig_thresh && significance[i] > cfg.objlim * f_norm
    };
    let primary: Vec<bool> = (0..w * h)
        .map(|i| !mask[i] && passes_contrast(i, cfg.sigclip))
        .collect();

    let lowered = cfg.sigclip * cfg.sigfrac;
    let mut flags = primary.clone();
    for y in 0..h {
        for x in 0..w {
            if !primary[y * w + x] {
                continue;
            }
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(h - 1);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    let j = ny * w + nx;
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
fn replace_flagged(data: &mut Buffer2<f32>, w: usize, h: usize, mask: &[bool]) {
    let src = data.pixels().to_vec();
    let (wi, hi) = (w as isize, h as isize);
    data.pixels_mut()
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
            let mut buf: Vec<f32> = Vec::with_capacity(25);
            for (x, o) in row.iter_mut().enumerate() {
                if !mask[y * w + x] {
                    continue;
                }
                buf.clear();
                for dy in -2..=2 {
                    let yy = (y as isize + dy).clamp(0, hi - 1) as usize;
                    for dx in -2..=2 {
                        let xx = (x as isize + dx).clamp(0, wi - 1) as usize;
                        let j = yy * w + xx;
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
    let size = Vec2us::new(data.width(), data.height());
    if size.x < 7 || size.y < 7 {
        return 0;
    }
    let mut mask = vec![false; size.x * size.y];

    for _ in 0..config.niter {
        let pix = data.pixels();
        let XtransStructure { lplus, f, signal } = xtrans_structure(pix, size, cfa, &mask);
        let noise = xtrans_noise(pix, size, cfa, &signal, &config.noise);
        let s: Vec<f32> = lplus.iter().zip(&noise).map(|(&l, &nz)| l / nz).collect();

        let flags = detect_and_grow(&s, &f, &noise, &mask, size.x, size.y, config);

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
    size: Vec2us,
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
    let my = scene.cfa.color_at(pos.x, pos.y);
    let w = scene.size.x;
    let (wi, hi) = (scene.size.x as i32, scene.size.y as i32);
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
            if scene.cfa.color_at(nxu, nyu) == my {
                out.push((dx.abs() + dy.abs(), scene.pix[nyu * w + nxu]));
            }
        }
    }
    out.sort_unstable_by_key(|&(d, _)| d);
    out.truncate(max);
}

/// Compute `L⁺`, `F`, and the signal estimate per pixel from same-color medians at two scales (one
/// gather per pixel: nearest-`XTRANS_LARGE`, with the nearest-`XTRANS_SMALL` subset).
fn xtrans_structure(pix: &[f32], size: Vec2us, cfa: &CfaType, mask: &[bool]) -> XtransStructure {
    let (w, n) = (size.x, size.x * size.y);
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
    size: Vec2us,
    cfa: &CfaType,
    signal: &[f32],
    noise: &NoiseEstimation,
) -> Vec<f32> {
    match *noise {
        NoiseEstimation::Empirical => {
            let mut by_color: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            for y in 0..size.y {
                for x in 0..size.x {
                    let c = (cfa.color_at(x, y) as usize).min(2);
                    by_color[c].push(pix[y * size.x + x]);
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
            (0..size.x * size.y)
                .map(|i| {
                    let p = Vec2us::from_index(i, size.x);
                    let c = (cfa.color_at(p.x, p.y) as usize).min(2);
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
    let size = Vec2us::new(data.width(), data.height());
    let w = size.x;
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
mod tests {
    use crate::io::astro_image::AstroImageMetadata;
    use crate::io::astro_image::cfa::CfaType;
    use crate::io::raw::demosaic::bayer::CfaPattern;
    use crate::stacking::calibration_masters::cosmic_ray::*;
    use crate::testing::TestRng;

    /// Add a round Gaussian source (peak above the existing background) at `(cx, cy)`.
    fn add_gaussian(data: &mut [f32], w: usize, h: usize, cx: f32, cy: f32, peak: f32, sigma: f32) {
        let r = (sigma * 4.0).ceil() as isize;
        let two_s2 = 2.0 * sigma * sigma;
        for dy in -r..=r {
            for dx in -r..=r {
                let x = cx as isize + dx;
                let y = cy as isize + dy;
                if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
                    continue;
                }
                let (xf, yf) = (x as f32 - cx, y as f32 - cy);
                data[y as usize * w + x as usize] += peak * (-(xf * xf + yf * yf) / two_s2).exp();
            }
        }
    }

    fn mono(data: Vec<f32>, w: usize, h: usize) -> CfaImage {
        CfaImage {
            data: Buffer2::new(w, h, data),
            metadata: AstroImageMetadata {
                cfa_type: Some(CfaType::Mono),
                ..Default::default()
            },
            quantization_sigma: None,
        }
    }

    /// 64×64: flat sky + deterministic Gaussian noise (σ≈0.003) + three well-sampled stars
    /// (FWHM≈3 px). Star centers are returned so a test can assert they survive.
    fn synthetic_field() -> (Vec<f32>, usize, usize, Vec<(usize, usize)>) {
        let (w, h) = (64, 64);
        let mut data = vec![0.05f32; w * h];
        let mut rng = TestRng::new(7);
        for v in data.iter_mut() {
            *v += rng.next_gaussian_f32() * 0.003;
        }
        let stars = [(20.0, 20.0, 0.6), (44.0, 30.0, 0.45), (32.0, 50.0, 0.7)];
        for &(cx, cy, peak) in &stars {
            add_gaussian(&mut data, w, h, cx, cy, peak, 1.3);
        }
        let centers = stars
            .iter()
            .map(|&(x, y, _)| (x as usize, y as usize))
            .collect();
        (data, w, h, centers)
    }

    #[test]
    fn removes_cosmic_rays_preserves_stars() {
        let (mut data, w, h, star_cores) = synthetic_field();
        // Single-pixel CRs at empty positions + a short horizontal streak.
        let crs = [(10usize, 10usize), (54, 12), (12, 54), (50, 50)];
        let streak = [(30usize, 8usize), (31, 8), (32, 8)];
        for &(x, y) in crs.iter().chain(&streak) {
            data[y * w + x] = 0.95;
        }
        let star_vals: Vec<f32> = star_cores.iter().map(|&(x, y)| data[y * w + x]).collect();

        let mut img = mono(data, w, h);
        let count = reject_cosmic_rays(&mut img, &CosmicRayConfig::default());
        let out = img.data.pixels();

        // Every injected CR is removed (the spike drops back toward sky, ≪ 0.95).
        for &(x, y) in crs.iter().chain(&streak) {
            assert!(
                out[y * w + x] < 0.2,
                "CR at ({x},{y}) not removed: {}",
                out[y * w + x]
            );
        }
        // Star cores are untouched — the fine-structure test must not flag PSF-broadened peaks.
        for (&(x, y), &orig) in star_cores.iter().zip(&star_vals) {
            assert!(
                (out[y * w + x] - orig).abs() < 1e-6,
                "star core at ({x},{y}) was altered: {} vs {orig}",
                out[y * w + x]
            );
        }
        // 7 injected; allow modest growth but not runaway over-flagging.
        assert!((7..=20).contains(&count), "unexpected CR count: {count}");
    }

    #[test]
    fn clean_field_few_false_positives() {
        let (data, w, h, _) = synthetic_field();
        let count = reject_cosmic_rays(&mut mono(data, w, h), &CosmicRayConfig::default());
        assert!(count <= 2, "clean field should flag ~0 CRs, got {count}");
    }

    #[test]
    fn sigclip_controls_sensitivity() {
        // A modest spike (~15σ): a sensitive sigclip flags it, a strict one doesn't (A→X, B→Y, X≠Y).
        let (mut data, w, h, _) = synthetic_field();
        data[40 * w + 40] = 0.05 + 0.045;
        let sensitive = reject_cosmic_rays(
            &mut mono(data.clone(), w, h),
            &CosmicRayConfig {
                sigclip: 3.0,
                ..Default::default()
            },
        );
        let strict = reject_cosmic_rays(
            &mut mono(data, w, h),
            &CosmicRayConfig {
                sigclip: 60.0,
                ..Default::default()
            },
        );
        assert!(
            sensitive > strict,
            "lower sigclip must flag more: sensitive={sensitive}, strict={strict}"
        );
    }

    #[test]
    fn empirical_and_parametric_both_catch_a_bright_cr() {
        // Both noise models must flag an obvious bright CR among the stars.
        let (mut data, w, h, _) = synthetic_field();
        data[33 * w + 15] = 0.99;
        for noise in [
            NoiseEstimation::Empirical,
            NoiseEstimation::Parametric {
                gain: 1.5,
                read_noise: 5.0,
                full_scale: 4095.0,
            },
        ] {
            let mut img = mono(data.clone(), w, h);
            let count = reject_cosmic_rays(
                &mut img,
                &CosmicRayConfig {
                    noise,
                    ..Default::default()
                },
            );
            assert!(count >= 1, "bright CR missed");
            assert!(
                img.data.pixels()[33 * w + 15] < 0.2,
                "bright CR not in-painted"
            );
        }
    }

    #[test]
    fn bayer_removes_cosmic_rays_preserves_star() {
        // Bayer deinterleave path: a well-sampled star + CRs spread across all four 2×2 phases. The
        // CRs go; the star core survives (each phase plane's mono detector protects it).
        let (w, h) = (48, 48);
        let mut data = vec![0.05f32; w * h];
        let mut rng = TestRng::new(11);
        for v in data.iter_mut() {
            *v += rng.next_gaussian_f32() * 0.003;
        }
        add_gaussian(&mut data, w, h, 24.0, 24.0, 0.6, 2.5);
        let star = data[24 * w + 24];
        // Each CR sits in a different (x%2, y%2) phase, exercising all four planes.
        let crs = [(8usize, 8usize), (9, 12), (12, 41), (37, 37)];
        for &(x, y) in &crs {
            data[y * w + x] = 0.95;
        }
        let mut img = CfaImage {
            data: Buffer2::new(w, h, data),
            metadata: AstroImageMetadata {
                cfa_type: Some(CfaType::Bayer(CfaPattern::Rggb)),
                ..Default::default()
            },
            quantization_sigma: None,
        };
        let count = reject_cosmic_rays(&mut img, &CosmicRayConfig::default());
        let out = img.data.pixels();
        for &(x, y) in &crs {
            assert!(
                out[y * w + x] < 0.2,
                "Bayer CR ({x},{y}) not removed: {}",
                out[y * w + x]
            );
        }
        assert!(
            out[24 * w + 24] > 0.5,
            "star core gutted: {} (was {star})",
            out[24 * w + 24]
        );
        assert!((4..=24).contains(&count), "unexpected CR count: {count}");
    }

    #[test]
    fn bayer_tight_star_eaten_is_a_known_limitation() {
        // CR-2 characterization (not a goal — a documented limitation). A *tight* star (FWHM≈2.35 px
        // in the mosaic → ~1.2 px in each half-res Bayer phase plane) is undersampled there: median₃
        // erases its core exactly like a cosmic ray, so the fine-structure test F→0 and L.A.Cosmic
        // *cannot* tell the core from a CR at any `objlim`. The per-phase detector therefore eats it.
        // This is why the module doc steers tight / dithered OSC data to stack-time σ-rejection
        // instead of per-frame CFA L.A.Cosmic. The test pins the behavior so a future fix
        // (e.g. mosaic-level detection) flips it loudly. Contrast `bayer_removes_cosmic_rays_...`,
        // which uses a *well-sampled* FWHM≈5.9 px star that survives.
        let (w, h) = (48, 48);
        let mut data = vec![0.05f32; w * h];
        let mut rng = TestRng::new(13);
        for v in data.iter_mut() {
            *v += rng.next_gaussian_f32() * 0.003;
        }
        add_gaussian(&mut data, w, h, 24.0, 24.0, 0.6, 1.0); // σ=1.0 → FWHM≈2.35 px in the mosaic
        let crs = [(8usize, 8usize), (37, 37)];
        for &(x, y) in &crs {
            data[y * w + x] = 0.95;
        }
        let mut img = CfaImage {
            data: Buffer2::new(w, h, data),
            metadata: AstroImageMetadata {
                cfa_type: Some(CfaType::Bayer(CfaPattern::Rggb)),
                ..Default::default()
            },
            quantization_sigma: None,
        };
        reject_cosmic_rays(&mut img, &CosmicRayConfig::default());
        let out = img.data.pixels();
        // CR rejection still works — the injected CRs are removed.
        for &(x, y) in &crs {
            assert!(
                out[y * w + x] < 0.2,
                "Bayer CR ({x},{y}) not removed: {}",
                out[y * w + x]
            );
        }
        // Known limitation: the tight star core is also gutted (flagged as a CR).
        assert!(
            out[24 * w + 24] < 0.2,
            "tight star core unexpectedly survived ({}) — if per-phase Bayer CR detection was \
             fixed, update this characterization test",
            out[24 * w + 24]
        );
    }

    #[test]
    fn xtrans_removes_cosmic_ray_preserves_flat_field() {
        // X-Trans same-color path: per-color baselines + tiny noise + one bright CR. The CR is
        // replaced from same-color neighbors (≈ its color's baseline); flat pixels stay put.
        let pattern = [
            [1, 0, 1, 1, 2, 1],
            [2, 1, 2, 0, 1, 0],
            [1, 2, 1, 1, 0, 1],
            [1, 2, 1, 1, 0, 1],
            [0, 1, 0, 2, 1, 2],
            [1, 0, 1, 1, 2, 1],
        ];
        let cfa = CfaType::XTrans(pattern);
        let (w, h) = (18usize, 18usize);
        let color_val = |c: u8| match c {
            0 => 0.10, // R
            1 => 0.20, // G
            _ => 0.30, // B
        };
        let mut data = vec![0.0f32; w * h];
        let mut rng = TestRng::new(5);
        for y in 0..h {
            for x in 0..w {
                data[y * w + x] = color_val(cfa.color_at(x, y)) + rng.next_gaussian_f32() * 0.002;
            }
        }
        let cr = (9usize, 9usize);
        data[cr.1 * w + cr.0] = 0.95;

        let mut img = CfaImage {
            data: Buffer2::new(w, h, data),
            metadata: AstroImageMetadata {
                cfa_type: Some(cfa.clone()),
                ..Default::default()
            },
            quantization_sigma: None,
        };
        let count = reject_cosmic_rays(&mut img, &CosmicRayConfig::default());
        let out = img.data.pixels();

        assert!(count >= 1, "X-Trans CR missed");
        // Replaced with the same-color (G, here) neighborhood median, ≈ 0.20 — well below the spike.
        let cr_color = cfa.color_at(cr.0, cr.1);
        assert!(
            (out[cr.1 * w + cr.0] - color_val(cr_color)).abs() < 0.05,
            "X-Trans CR not repaired to its color baseline: {}",
            out[cr.1 * w + cr.0]
        );
        // A flat pixel of each color far from the CR is untouched (no false positives).
        for &(x, y) in &[(3usize, 3usize), (4, 3), (3, 4)] {
            let c = cfa.color_at(x, y);
            assert!(
                (out[y * w + x] - color_val(c)).abs() < 0.02,
                "flat {c}-pixel ({x},{y}) altered: {}",
                out[y * w + x]
            );
        }
    }
}
