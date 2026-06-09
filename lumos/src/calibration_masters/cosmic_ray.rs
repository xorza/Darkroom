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
//! **Phase 1 is monochrome only.** The Laplacian and median stencils assume same-color orthogonal
//! neighbors, true only for `CfaType::Mono`; same-color stencils for Bayer/X-Trans are Phase 2.

use common::Buffer2;
use rayon::prelude::*;

use crate::astro_image::cfa::CfaImage;
use crate::math::statistics::{mad_f32_fast, mad_to_sigma, median_f32_mut};

/// `F` is floored to this (in normalized pixel units) so the CR-to-fine-structure contrast test
/// `L⁺ > objlim·F` stays well-defined where the object fine structure is ~0 (i.e. at a CR).
const FINE_STRUCTURE_FLOOR: f32 = 1e-6;

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

/// Per-pixel noise model for the significance image `S = L⁺ / (2·N)`.
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

/// Detect and in-paint cosmic rays in a single calibrated mono frame, in place. Returns the number
/// of CR pixels corrected.
///
/// Callers must gate on `CfaType::Mono` until Phase 2 adds CFA stencils.
pub(crate) fn reject_cosmic_rays(image: &mut CfaImage, config: &CosmicRayConfig) -> usize {
    let w = image.data.width();
    let h = image.data.height();
    if w < 3 || h < 3 {
        return 0;
    }
    let n = w * h;
    let mut mask = vec![false; n];

    for _ in 0..config.niter {
        let data = image.data.pixels();

        // L⁺: clipped Laplacian of the ×2-subsampled frame, averaged back to native resolution.
        let sub = subsample2(data, w, h);
        let lplus = laplacian_plus(&sub, w * 2, h * 2, w, h);

        // Object fine structure F = median₃(I) − median₇(median₃(I)); large for real sources, ~0 at
        // a CR (median₃ already erased the spike).
        let m3 = median_window(data, w, h, 1);
        let m37 = median_window(&m3, w, h, 3);
        let f: Vec<f32> = m3
            .iter()
            .zip(&m37)
            .map(|(&a, &b)| (a - b).max(FINE_STRUCTURE_FLOOR))
            .collect();

        // Significance S = L⁺/(2N), then S' = S − median₅(S) to strip smooth large-scale structure.
        let m5 = median_window(data, w, h, 2);
        let noise = noise_map(data, &m5, &config.noise);
        let s: Vec<f32> = lplus
            .iter()
            .zip(&noise)
            .map(|(&l, &nz)| l / (2.0 * nz))
            .collect();
        let s_med5 = median_window(&s, w, h, 2);
        let sprime: Vec<f32> = s.iter().zip(&s_med5).map(|(&a, &b)| a - b).collect();

        let flags = detect_and_grow(&lplus, &sprime, &f, &mask, w, h, config);

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
        replace_flagged(&mut image.data, w, h, &mask);
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
            let sigma2 = sigma_bg * sigma_bg;
            // Poisson slope anchored at the background: N = σ_bg·√(signal/bg) above sky.
            let slope = sigma2 / bg.max(sigma_bg);
            m5.iter()
                .map(|&s| (sigma2 + (s - bg).max(0.0) * slope).sqrt())
                .collect()
        }
        NoiseEstimation::Parametric {
            gain,
            read_noise,
            full_scale,
        } => {
            let denom = gain * full_scale;
            m5.iter()
                .map(|&s| {
                    let adu = s.max(0.0) * full_scale;
                    let var_e = gain * adu + read_noise * read_noise;
                    (var_e.sqrt() / denom).max(1e-9)
                })
                .collect()
        }
    }
}

/// Flag CRs: `S' > sigclip` **and** `L⁺ > objlim·F`, then grow onto neighbors that clear the lowered
/// threshold `sigclip·sigfrac` and the same contrast test (a flagged CR's fainter wings).
fn detect_and_grow(
    lplus: &[f32],
    sprime: &[f32],
    f: &[f32],
    mask: &[bool],
    w: usize,
    h: usize,
    cfg: &CosmicRayConfig,
) -> Vec<bool> {
    let passes_contrast =
        |i: usize, sig_thresh: f32| sprime[i] > sig_thresh && lplus[i] > cfg.objlim * f[i];
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::astro_image::AstroImageMetadata;
    use crate::astro_image::cfa::CfaType;
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
}
