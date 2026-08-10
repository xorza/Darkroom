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

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
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
    let size = Size2us::new(image.data.width(), image.data.height());
    // Disjoint fields: the pixels go in by `&mut`, the CFA type is read from the metadata beside it.
    let pixels = image.data.pixels_mut();
    match &image.metadata.cfa_type {
        // Bayer is 2×2-periodic → four dense same-color planes; reuse the mono detector per plane.
        Some(CfaType::Bayer(_)) => reject_bayer(pixels, size, config),
        // X-Trans has no dense same-color sub-lattice → same-color stencils on the mosaic.
        Some(c @ CfaType::XTrans(_)) => reject_xtrans(pixels, size, c, config),
        // Mono (or an unlabeled frame): the dense Laplacian path.
        _ => reject_mono_buffer(pixels, size, config, &mut MonoScratch::default()),
    }
}

/// The mono detector's frame-sized `f32` working set, allocated on the first iteration and reused
/// by every one after it.
///
/// Each buffer is written in full before it is read, so what the previous iteration — or the
/// previous Bayer plane — left in it never matters; only the capacity does. The four Bayer phase
/// planes differ by at most a row and a column, so one scratch serves all four: the `resize` in
/// each producer keeps the largest allocation.
///
/// Five planes, not the eight the stages name: `significance` and `fine` are rewritten in place by
/// the elementwise step that consumes them, and `median` and `frame` are each handed from one
/// stage to the next. On a 6144² mono frame that is 720 MB of working set instead of 1.1 GB —
/// and, the point of the struct, no allocation at all after the first iteration.
#[derive(Debug, Default)]
struct MonoScratch {
    /// `L⁺`, then the significance `S = L⁺/(2N)`, then `S' = S − median₅(S)`, each in place.
    significance: Vec<f32>,
    /// `median₃(I)`, then the object fine structure `F = median₃ − median₇(median₃)` in place.
    fine: Vec<f32>,
    /// Per-pixel noise `N`.
    noise: Vec<f32>,
    /// The window medians, one at a time: `median₇(median₃(I))`, then `median₅(I)`, then
    /// `median₅(S)`. Each is consumed by the step immediately after it, so the three never overlap.
    median: Vec<f32>,
    /// Whole-frame scratch: the copy the empirical background median and MAD consume, then the
    /// read-only snapshot [`replace_flagged`] gathers from — again never both at once.
    frame: Vec<f32>,
}

/// Monochrome L.A.Cosmic on one plane (also each deinterleaved Bayer plane). Subsample ×2 → clipped
/// Laplacian → resample → significance `S = L⁺/(2N)` → `S' = S − median₅(S)` → fine structure `F`
/// → flag → grow → in-paint → iterate. Returns the CR pixel count.
fn reject_mono_buffer(
    data: &mut [f32],
    size: Size2us,
    config: &CosmicRayConfig,
    scratch: &mut MonoScratch,
) -> usize {
    debug_assert_eq!(data.len(), size.pixel_count());
    if size.width < 3 || size.height < 3 {
        return 0;
    }
    let mut masks = CrMasks::new(size);

    for _ in 0..config.niter {
        let MonoScratch {
            significance,
            fine,
            noise,
            median,
            frame,
        } = &mut *scratch;
        let pix = &*data;

        // L⁺: clipped Laplacian of the ×2-subsampled frame, averaged back to native resolution.
        laplacian_plus_into(pix, size, significance);

        // Object fine structure F = median₃(I) − median₇(median₃(I)); large for real sources, ~0 at
        // a CR (median₃ already erased the spike). The difference is elementwise, so it lands back
        // over median₃ in `fine` rather than in a buffer of its own.
        median_window_into(pix, size, 1, fine);
        median_window_into(fine, size, 3, median);
        for (a, &b) in fine.iter_mut().zip(&*median) {
            *a = (*a - b).max(FINE_STRUCTURE_FLOOR);
        }

        // Significance S = L⁺/(2N), then S' = S − median₅(S) to strip smooth large-scale structure.
        // Both steps are elementwise over the same extent, so they run in place down the Laplacian
        // buffer instead of allocating a frame each.
        median_window_into(pix, size, 2, median);
        noise_map_into(pix, median, &config.noise, noise, frame);
        for (l, &nz) in significance.iter_mut().zip(&*noise) {
            *l /= 2.0 * nz;
        }
        median_window_into(significance, size, 2, median);
        for (v, &m) in significance.iter_mut().zip(&*median) {
            *v -= m;
        }

        if masks.detect_and_grow(significance, fine, noise, config) == 0 {
            break;
        }
        replace_flagged(data, size, &masks.accumulated, frame);
    }

    masks.accumulated.count_ones()
}

/// The three cosmic-ray masks a detection holds, one bit per pixel each.
///
/// All three live for the whole detection rather than being rebuilt per iteration, which costs
/// nothing at the peak — [`detect_and_grow`](Self::detect_and_grow) needed all three live at once
/// anyway — and saves two frame-sized allocations per pass.
#[derive(Debug)]
struct CrMasks {
    /// Every CR pixel found so far: the in-painting mask, and the count the detector returns.
    accumulated: BitBuffer2,
    /// Pixels clearing the full `sigclip` this iteration, before growth.
    primary: BitBuffer2,
    /// `primary` plus the wings grown onto it — what merges into `accumulated`.
    flags: BitBuffer2,
}

impl CrMasks {
    fn new(size: Size2us) -> Self {
        Self {
            accumulated: new_cr_mask(size),
            primary: new_cr_mask(size),
            flags: new_cr_mask(size),
        }
    }

    /// Flag CRs: `S' > sigclip` **and** the fine-structure contrast `S' > objlim·(F/noise)`, then
    /// grow onto neighbors clearing the lowered threshold `sigclip·sigfrac` and the same contrast
    /// test (a flagged CR's fainter wings). Merges the result into `accumulated` and returns how
    /// many pixels that added — zero ends the detect→replace loop.
    ///
    /// The contrast is van Dokkum's `L⁺/F > objlim` written in astroscrappy's noise-normalized
    /// form: comparing the significance image `S'` against `objlim·(F/noise)` (rather than raw `L⁺`
    /// against `objlim·F`) puts `F` in the same units as `S'`, so the `objlim` default carries the
    /// same star-core protection as astroscrappy/ccdproc. (Raw `L⁺ > objlim·F` is ~2× more
    /// aggressive.)
    fn detect_and_grow(
        &mut self,
        significance: &[f32],
        f: &[f32],
        noise: &[f32],
        cfg: &CosmicRayConfig,
    ) -> usize {
        let Self {
            accumulated,
            primary,
            flags,
        } = self;
        let size = accumulated.size;
        let passes_contrast = |i: usize, sig_thresh: f32| {
            let f_norm = (f[i] / noise[i]).max(FINE_STRUCTURE_SIGMA_FLOOR);
            significance[i] > sig_thresh && significance[i] > cfg.objlim * f_norm
        };
        primary.fill_from_predicate(|i| !accumulated.get(i) && passes_contrast(i, cfg.sigclip));

        let lowered = cfg.sigclip * cfg.sigfrac;
        flags.copy_from(primary);
        for y in 0..size.height {
            for x in 0..size.width {
                if !primary.get_at(Vec2us::new(x, y)) {
                    continue;
                }
                let y0 = y.saturating_sub(1);
                let y1 = (y + 1).min(size.height - 1);
                let x0 = x.saturating_sub(1);
                let x1 = (x + 1).min(size.width - 1);
                for ny in y0..=y1 {
                    for nx in x0..=x1 {
                        let j = size.index_of(Vec2us::new(nx, ny));
                        if !flags.get(j) && !accumulated.get(j) && passes_contrast(j, lowered) {
                            flags.set(j, true);
                        }
                    }
                }
            }
        }

        // Word-wise: `flags & !accumulated` is what is newly set, then `accumulated |= flags`.
        // Counting whole words needs no per-row masking only because both buffers have their
        // padding clear.
        debug_assert!(
            accumulated.padding_is_clear() && flags.padding_is_clear(),
            "padding bits would be counted as newly-flagged pixels"
        );
        let mut newly = 0usize;
        for (acc, &new) in accumulated.words.iter_mut().zip(&flags.words) {
            newly += (new & !*acc).count_ones() as usize;
            *acc |= new;
        }
        newly
    }
}

/// One cosmic-ray mask: one bit per pixel.
///
/// Its own function so `mem_budget` can weigh exactly what the detector allocates. A detection
/// holds three ([`CrMasks`]), so the packing is worth three times its face value.
fn new_cr_mask(size: Size2us) -> BitBuffer2 {
    BitBuffer2::new_default(size)
}

/// Clipped Laplacian of the ×2-subsampled frame, block-averaged back down to `size`.
///
/// Convolves the ×2 image with `[[0,−1,0],[−1,4,−1],[0,−1,0]]`, clips negatives to 0 (keeping only
/// sharp positive peaks), then averages each 2×2 block. Edge-clamped on the ×2 grid.
///
/// The ×2 image is never materialized. Subsampling here is a block replication — every `sub`
/// sample is `data[y2 / 2][x2 / 2]` — so it is read through that index instead of being written to
/// a buffer four times pixel count, and the clipped Laplacian is averaged as it is produced rather
/// than stored in a second buffer of the same size. That is 8n floats of allocation and traffic
/// removed from every iteration; what remains is the `n`-length result, written into the caller's
/// buffer.
fn laplacian_plus_into(data: &[f32], size: Size2us, out: &mut Vec<f32>) {
    let (w2, h2) = (size.width * 2, size.height * 2);
    // The ×2 sample at (x2, y2), which is the native pixel under it.
    let at = |y2: usize, x2: usize| data[(y2 / 2) * size.width + (x2 / 2)];

    // Every element is written below, so only the length matters.
    out.resize(size.pixel_count(), 0.0);
    out.par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let mut sum = 0.0f32;
                for dy in 0..2 {
                    let y2 = 2 * y + dy;
                    let yu = y2.saturating_sub(1);
                    let yd = (y2 + 1).min(h2 - 1);
                    for dx in 0..2 {
                        let x2 = 2 * x + dx;
                        let xl = x2.saturating_sub(1);
                        let xr = (x2 + 1).min(w2 - 1);
                        let v =
                            4.0 * at(y2, x2) - at(yu, x2) - at(yd, x2) - at(y2, xl) - at(y2, xr);
                        sum += v.max(0.0);
                    }
                }
                *o = 0.25 * sum;
            }
        });
}

/// Median over a `(2r+1)²` window, replicating the border pixel for out-of-bounds coordinates so
/// every output sees a full window. Scalar, row-parallel.
///
/// Deliberately not [`median_filter_3x3`](crate::stacking::star_detection::median_filter), even
/// though `r == 1` describes the same 3×3 median: that one *shrinks* its window at the border to
/// the 4 or 6 in-bounds samples where this one replicates. L.A.Cosmic differences two of these
/// windows against each other — the fine structure is `median₃ − median₇(median₃)` — so every
/// radius here has to share one border convention. Feeding a shrunk `median₃` into that
/// difference while `median₇` stayed replicated would corrupt the border in a way neither
/// convention does alone. Replication is also the usual choice for astronomical median filtering.
///
/// The two *could* still share an interior kernel: `median_filter`'s `median_filter_row_simd`
/// takes three rows and fills the interior, knowing nothing about edges. That is worth doing
/// behind a cosmic-ray benchmark rather than before one — it couples two subsystems to accelerate
/// one of the four windows below (areas 9, 49, 25, 25), and an `r == 1` fast path would have to
/// be proven bit-identical to this general one or the detection changes.
fn median_window_into(data: &[f32], size: Size2us, r: usize, out: &mut Vec<f32>) {
    let ri = r as isize;
    let (wi, hi) = (size.width as isize, size.height as isize);
    // Every element is written below, so only the length matters.
    out.resize(size.pixel_count(), 0.0);
    out.par_chunks_mut(size.width).enumerate().for_each_init(
        || Vec::<f32>::with_capacity((2 * r + 1) * (2 * r + 1)),
        |buf, (y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                buf.clear();
                for dy in -ri..=ri {
                    let yy = (y as isize + dy).clamp(0, hi - 1) as usize;
                    for dx in -ri..=ri {
                        let xx = (x as isize + dx).clamp(0, wi - 1) as usize;
                        buf.push(data[size.index_of(Vec2us::new(xx, yy))]);
                    }
                }
                *o = median_f32_mut(buf);
            }
        },
    );
}

/// Per-pixel noise `N` from the median-filtered (CR-free) signal estimate `m5`, into `out`.
///
/// `scratch` is a frame-sized buffer the empirical background statistics consume; what it holds on
/// entry means nothing, and what it holds on return means nothing either.
fn noise_map_into(
    data: &[f32],
    m5: &[f32],
    noise: &NoiseEstimation,
    out: &mut Vec<f32>,
    scratch: &mut Vec<f32>,
) {
    match *noise {
        NoiseEstimation::Empirical => {
            scratch.clear();
            scratch.extend_from_slice(data);
            let bg = median_f32_mut(scratch);
            let sigma_bg = mad_to_sigma(mad_f32_fast(data, bg, scratch)).max(1e-9);
            out.clear();
            out.extend(m5.iter().map(|&s| empirical_noise(s, bg, sigma_bg)));
        }
        NoiseEstimation::Parametric {
            gain,
            read_noise,
            full_scale,
        } => parametric_noise_into(m5, gain, read_noise, full_scale, out),
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
fn parametric_noise_into(
    signal: &[f32],
    gain: f32,
    read_noise: f32,
    full_scale: f32,
    out: &mut Vec<f32>,
) {
    let denom = gain * full_scale;
    out.clear();
    out.extend(signal.iter().map(|&s| {
        let adu = s.max(0.0) * full_scale;
        ((gain * adu + read_noise * read_noise).sqrt() / denom).max(1e-9)
    }));
}

/// Replace masked pixels with the median of their unmasked 5×5 neighbors (edge-clamped);
/// fully-masked neighborhoods (huge CRs) are left for the next iteration to shrink.
///
/// The frame copy is not what makes replacements independent of each other — writes land only on
/// masked pixels and reads only on unmasked ones, so no replacement can consult a replaced
/// neighbour whatever order the rows run in. It is here because `pixels_mut()` is held across the
/// reads, and it pays for itself besides: reading a separate, read-only array keeps one row's
/// writes off the cache lines the rows above and below are reading. Aliasing the two through a raw
/// pointer is sound (the sets are disjoint) and was measured *slower* on every run of
/// `bench_cosmic_ray_reject_mono` — the false sharing costs more than the copy. The copy lands in
/// the caller's `snapshot` buffer, so it is a memcpy per iteration and not an allocation.
fn replace_flagged(data: &mut [f32], size: Size2us, mask: &BitBuffer2, snapshot: &mut Vec<f32>) {
    snapshot.clear();
    snapshot.extend_from_slice(data);
    let src: &[f32] = snapshot;
    let (wi, hi) = (size.width as isize, size.height as isize);
    data.par_chunks_mut(size.width).enumerate().for_each_init(
        || Vec::<f32>::with_capacity(25),
        |buf, (y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                if !mask.get_at(Vec2us::new(x, y)) {
                    continue;
                }
                buf.clear();
                for dy in -2..=2 {
                    let yy = (y as isize + dy).clamp(0, hi - 1) as usize;
                    for dx in -2..=2 {
                        let xx = (x as isize + dx).clamp(0, wi - 1) as usize;
                        let j = size.index_of(Vec2us::new(xx, yy));
                        if !mask.get(j) {
                            buf.push(src[j]);
                        }
                    }
                }
                if !buf.is_empty() {
                    *o = median_f32_mut(buf);
                }
            }
        },
    );
}

/// Bayer: the mosaic is 2×2-periodic, so pixels sharing a `(x%2, y%2)` phase are the same color and
/// form a dense plane. Deinterleave the four phases, run [`reject_mono_buffer`] on each (its dense
/// neighbors are same-color in the mosaic), and write the cleaned planes back. Pattern-independent —
/// phase alone determines color, so no `CfaPattern` is needed.
///
/// One [`MonoScratch`] and one `plane` serve all four phases: `(0, 0)` is the largest, and it runs
/// first, so no later phase grows either allocation.
///
/// Deinterleave and re-interleave are row-parallel like the detector between them. They are only a
/// few percent of a frame today, but they are the whole of its *serial* fraction — the one part
/// that would not shrink as thread count rises.
fn reject_bayer(data: &mut [f32], size: Size2us, config: &CosmicRayConfig) -> usize {
    let (w, h) = (size.width, size.height);
    let mut scratch = MonoScratch::default();
    let mut plane: Vec<f32> = Vec::new();
    let mut total = 0;
    for b in 0..2 {
        for a in 0..2 {
            let pw = if a == 0 { w.div_ceil(2) } else { w / 2 };
            let ph = if b == 0 { h.div_ceil(2) } else { h / 2 };
            if pw < 3 || ph < 3 {
                continue;
            }

            // Deinterleave: plane row j is mosaic row 2j+b, every second pixel from column a. Every
            // element is written, so `resize` only has to get the length right.
            plane.resize(pw * ph, 0.0);
            let mosaic = &*data;
            plane.par_chunks_mut(pw).enumerate().for_each(|(j, row)| {
                let src = &mosaic[(j * 2 + b) * w..][..w];
                for (i, o) in row.iter_mut().enumerate() {
                    *o = src[i * 2 + a];
                }
            });

            total += reject_mono_buffer(&mut plane, Size2us::new(pw, ph), config, &mut scratch);

            // Re-interleave the cleaned plane. Chunking the mosaic by row keeps each thread's
            // writes to one row, so the phase's rows can be picked out of the full sweep.
            let cleaned = &plane[..];
            data.par_chunks_mut(w)
                .enumerate()
                .filter(|(y, _)| y % 2 == b)
                .for_each(|(y, row)| {
                    for (i, &v) in cleaned[(y / 2) * pw..][..pw].iter().enumerate() {
                        row[i * 2 + a] = v;
                    }
                });
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

/// The CFA detector's per-pixel inputs and the scratch that builds them, allocated on the first
/// iteration and reused by every one after it — [`MonoScratch`]'s rule on the X-Trans path.
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
fn reject_xtrans(
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
/// from a snapshot in the caller's `snapshot` buffer, for the reason [`replace_flagged`] gives.
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

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod internals {
    use crate::bit_buffer2::BitBuffer2;
    use crate::math::size2us::Size2us;
    use crate::stacking::calibration_masters::cosmic_ray::{CosmicRayConfig, MonoScratch};

    /// Masks a detection holds: the accumulated one, plus `primary` and `flags`.
    pub(crate) const CONCURRENT_MASKS: usize = 3;

    /// Frame-sized `f32` planes the mono detector holds, however many iterations it runs.
    pub(crate) const MONO_SCRATCH_PLANES: usize = 5;

    /// The mask as the detector allocates it, for `mem_budget` to weigh.
    pub(crate) fn new_cr_mask(size: Size2us) -> BitBuffer2 {
        super::new_cr_mask(size)
    }

    /// Total capacity, in floats, of the mono detector's working set after a run on `data` — what
    /// `mem_budget` weighs against [`MONO_SCRATCH_PLANES`].
    ///
    /// Destructured rather than summed through a helper, so a plane added to or dropped from
    /// [`MonoScratch`] fails to compile here instead of silently drifting from the constant.
    pub(crate) fn mono_scratch_floats(
        data: &mut [f32],
        size: Size2us,
        config: &CosmicRayConfig,
    ) -> usize {
        let mut scratch = MonoScratch::default();
        super::reject_mono_buffer(data, size, config, &mut scratch);
        let MonoScratch {
            significance,
            fine,
            noise,
            median,
            frame,
        } = &scratch;
        significance.capacity()
            + fine.capacity()
            + noise.capacity()
            + median.capacity()
            + frame.capacity()
    }
}
