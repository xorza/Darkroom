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
//! **CFA caveat:** L.A.Cosmic assumes a PSF-sampled image, but a Bayer phase plane is
//! half-resolution — a tight star (FWHM ≲ 2–3 px in the mosaic) becomes ~1 px there, where the
//! CR-vs-star fine-structure test weakens. This per-frame rejection is therefore best for **short,
//! un-dithered** sequences; for dithered sets prefer dither + stack-time σ/winsor rejection, which
//! out-votes CRs without a per-frame discriminator. (`xtrans_removes_cosmic_ray...` /
//! `bayer_tight_star...` tests pin the tight-star behavior.)

mod bayer;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod masks;
pub(crate) mod mono;
pub(crate) mod noise_model;
pub(crate) mod xtrans;

use crate::background_mesh::colour_mesh::ColourMesh;
use crate::background_mesh::workspace::MeshWorkspace;
use crate::bit_buffer2::BitBuffer2;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::math::size2us::Size2us;

use crate::calibration_masters::cosmic_ray::bayer::BayerDetector;
use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::calibration_masters::cosmic_ray::error::UnknownAdcStep;
use crate::calibration_masters::cosmic_ray::mono::MonoDetector;
use crate::calibration_masters::cosmic_ray::noise_model::{NoiseModel, PixelBackgrounds};
use crate::calibration_masters::cosmic_ray::xtrans::XtransDetector;

/// The tile side of the mesh the local background comes from: as the dark-current model's, small
/// beside a sensor's gradients and amp glow, large enough for a robust sky per colour.
const BACKGROUND_TILE_SIZE: usize = 64;

/// Floor for the **noise-normalized** fine structure `F/noise` in the contrast test (in σ units).
/// Matches astroscrappy's `f.clip(min=0.01)` — bounds the `S'/(F/noise)` ratio where fine structure
/// is ~0 so a CR (F→0) doesn't divide by zero.
const FINE_STRUCTURE_SIGMA_FLOOR: f32 = 0.01;

/// Detect and in-paint cosmic rays in a single calibrated frame, in place, dispatching on its CFA
/// type (mono / Bayer / X-Trans), and flag every in-painted pixel [`QualityFlags::COSMIC_RAY`] and
/// [`QualityFlags::REPAIRED`]. Returns the number of CR pixels corrected, or an error when the
/// parametric noise model needs an ADC step the frame does not record.
pub(crate) fn reject_cosmic_rays(
    image: &mut CfaImage,
    config: &CosmicRayConfig,
) -> Result<usize, UnknownAdcStep> {
    let noise = NoiseModel::resolve(&config.noise, &image.metadata)?;
    let size = Size2us::new(image.data.width(), image.data.height());
    // Measured once, before any repair: a hit spoils a tile's statistics no more than a star does,
    // and the mesh's clip and median filter are what keep both out.
    let mesh = ColourMesh::measure(
        &image.data,
        &image.cfa_type,
        BACKGROUND_TILE_SIZE,
        &mut MeshWorkspace::default(),
    );
    // Through the checked accessor, then shared: the pixels below are borrowed apart from it.
    let flat_gain = image.flat_gain().and(image.metadata.flat_gain.clone());
    let backgrounds = PixelBackgrounds {
        mesh: &mesh,
        flat_gain: flat_gain.as_deref(),
    };
    // Disjoint fields: the pixels go in by `&mut`, the CFA type is read beside them.
    let pixels = image.data.pixels_mut();
    let mut found = BitBuffer2::new_default(size);
    let count = match &image.cfa_type {
        // Bayer is 2×2-periodic → four dense same-color planes; reuse the mono detector per plane.
        c @ CfaType::Bayer(_) => {
            BayerDetector::new(config, noise, c).reject(pixels, size, &backgrounds, &mut found)
        }
        // X-Trans has no dense same-color sub-lattice → same-color stencils on the mosaic.
        c @ CfaType::XTrans(_) => {
            XtransDetector::new(config, noise, c).reject(pixels, size, &backgrounds, &mut found)
        }
        CfaType::Mono => MonoDetector::new(config, noise).reject(
            pixels,
            size,
            &|index| backgrounds.at(0, size.point_of(index)),
            &mut found,
        ),
    };
    if count > 0 {
        PixelFlags::add_where(
            &mut image.flags,
            size,
            QualityFlags::COSMIC_RAY.union(QualityFlags::REPAIRED),
            |index| found.get(index),
        );
    }
    Ok(count)
}

/// The bytes a cosmic-ray pass over a `size` mosaic of `cfa_type` allocates beside the mosaic, at
/// its peak: the frame-sized planes and masks. The background mesh is left out: a few tiles per
/// colour, and one tile's samples per worker, about 20 KB each, under a thousandth of the planes
/// of any frame large enough to plan for.
pub(crate) fn heap_bytes(cfa_type: &CfaType, size: Size2us) -> usize {
    match cfa_type {
        CfaType::Bayer(_) => BayerDetector::heap_bytes(size),
        CfaType::XTrans(_) => XtransDetector::heap_bytes(size),
        CfaType::Mono => MonoDetector::heap_bytes(size),
    }
}

#[cfg(test)]
mod tests;
