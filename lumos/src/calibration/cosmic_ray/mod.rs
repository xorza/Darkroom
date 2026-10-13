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
//! four 2×2 phases and reuse the mono detector per dense same-color plane. **X-Trans** is refused
//! ([`CosmicRayError::XTransUnsupported`]): it has no dense same-colour sub-lattice, and its red
//! and blue photosites have no point-symmetric same-colour pair nearer than √18 px, so no second
//! difference separates a hit from a star's convex wing at the scale of a star. No reference tool
//! runs L.A.Cosmic on an X-Trans mosaic, and Siril refuses its CFA cosmetic correction there.
//! X-Trans data relies on dithering and stack-time rejection.
//!
//! **CFA caveat:** L.A.Cosmic assumes a PSF-sampled image, but a Bayer phase plane is
//! half-resolution — a tight star (FWHM ≲ 2–3 px in the mosaic) becomes ~1 px there, where the
//! CR-vs-star fine-structure test weakens. This per-frame rejection is therefore best for **short,
//! un-dithered** sequences; for dithered sets prefer dither + stack-time σ/winsor rejection, which
//! out-votes CRs without a per-frame discriminator. (The `bayer_tight_star...` test pins the
//! tight-star behavior.)

mod bayer;
pub(crate) mod cosmic_ray_config;
pub(crate) mod error;
pub(crate) mod kept_out;
pub(crate) mod masks;
pub(crate) mod mono;
pub(crate) mod noise_model;

use crate::background_mesh::colour_mesh::ColourMesh;
use crate::background_mesh::mesh_workspace::MeshWorkspace;
use crate::bit_buffer2::BitBuffer2;
use crate::frame::stackable_image::StackableImage;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::quality_flags::QualityFlags;
use crate::math::size2us::Size2us;

use crate::calibration::cosmic_ray::bayer::BayerDetector;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::calibration::cosmic_ray::error::CosmicRayError;
use crate::calibration::cosmic_ray::kept_out::KeptOut;
use crate::calibration::cosmic_ray::mono::MonoDetector;
use crate::calibration::cosmic_ray::noise_model::{NoiseModel, PixelBackgrounds};

/// The tile side of the mesh the local background comes from: as the dark-current model's, small
/// beside a sensor's gradients and amp glow, large enough for a robust sky per colour.
const BACKGROUND_TILE_SIZE: usize = 64;

/// Floor for the **noise-normalized** fine structure `F/noise` in the contrast test (in σ units).
/// Matches astroscrappy's `f.clip(min=0.01)` — bounds the `S'/(F/noise)` ratio where fine structure
/// is ~0 so a CR (F→0) doesn't divide by zero.
const FINE_STRUCTURE_SIGMA_FLOOR: f32 = 0.01;

/// Frame-sized masks a pass holds beside its detector's: [`KeptOut`]'s two and the mask of the
/// pixels it repaired, or, before that mask exists, the three that building `KeptOut` holds at
/// once.
const PASS_MASKS: usize = 3;

/// The detector a CFA type gets: one dense plane, or four Bayer phases. X-Trans gets none.
#[derive(Debug, Clone, Copy)]
enum Layout {
    Mono,
    Bayer(CfaPattern),
}

impl Layout {
    const fn of(cfa_type: &CfaType) -> Result<Self, CosmicRayError> {
        match cfa_type {
            CfaType::Mono => Ok(Self::Mono),
            CfaType::Bayer(pattern) => Ok(Self::Bayer(*pattern)),
            CfaType::XTrans(_) => Err(CosmicRayError::XTransUnsupported),
        }
    }
}

/// Detect and in-paint cosmic rays in a single calibrated frame, in place, dispatching on its CFA
/// type (mono / Bayer), and flag every in-painted pixel [`QualityFlags::COSMIC_RAY`] and
/// [`QualityFlags::REPAIRED`]. Saturated star cores and pixels with no measurement are kept out of
/// detection, the background and every in-paint's sources, as [`KeptOut`] says. Returns the number
/// of CR pixels corrected, or an error, with the frame untouched, for an X-Trans mosaic or when the
/// parametric noise model needs an ADC step the frame does not record.
pub(crate) fn reject(
    image: &mut CfaImage,
    config: &CosmicRayConfig,
) -> Result<usize, CosmicRayError> {
    let layout = Layout::of(&image.cfa_type)?;
    let noise = NoiseModel::resolve(&config.noise, &image.metadata)?;
    let size = Size2us::new(image.data.width(), image.data.height());
    let kept_out = KeptOut::of(&image.data, image.flags.as_ref());
    // Measured once, before any repair: a hit spoils a tile's statistics no more than a star does,
    // and the mesh's clip and median filter are what keep both out.
    let mesh = ColourMesh::measure(
        &image.data,
        &image.cfa_type,
        BACKGROUND_TILE_SIZE,
        kept_out.as_ref().map(|kept_out| &kept_out.never_flagged),
        &mut MeshWorkspace::default(),
    );
    // Through the checked accessor, then shared: the pixels below are borrowed apart from it.
    let flat_gain = image
        .flat_gain()
        .and_then(|_| image.metadata.flat_gain.clone());
    let backgrounds = PixelBackgrounds {
        mesh: &mesh,
        flat_gain: flat_gain.as_deref(),
    };
    let pixels = image.data.pixels_mut();
    let mut found = BitBuffer2::new_default(size);
    let count = match layout {
        // Bayer is 2×2-periodic → four dense same-color planes; reuse the mono detector per plane.
        Layout::Bayer(pattern) => BayerDetector::new(config, noise, pattern).reject(
            pixels,
            size,
            &backgrounds,
            kept_out.as_ref(),
            &mut found,
        ),
        Layout::Mono => MonoDetector::new(config, noise).reject(
            pixels,
            size,
            &|index| backgrounds.at(0, size.point_of(index)),
            kept_out.as_ref(),
            &mut found,
        ),
    };
    if count > 0 {
        image.add_flags_where(
            QualityFlags::COSMIC_RAY.union(QualityFlags::REPAIRED),
            |index| found.get(index),
        );
    }
    Ok(count)
}

/// The bytes a cosmic-ray pass over a `size` mosaic of `cfa_type` allocates beside the mosaic, at
/// its peak: the detector's planes and masks, and the pass's own [`PASS_MASKS`]. Left out are the
/// fixed sizes: the background mesh, a few tiles per colour and one tile's samples per worker,
/// about 20 KB each, and the 256 KiB histogram the saturation level is ranked in.
///
/// # Errors
///
/// [`CosmicRayError::XTransUnsupported`] for an X-Trans mosaic, which the pass refuses.
pub(crate) fn heap_bytes(cfa_type: &CfaType, size: Size2us) -> Result<usize, CosmicRayError> {
    let detector = match Layout::of(cfa_type)? {
        Layout::Bayer(_) => BayerDetector::heap_bytes(size),
        Layout::Mono => MonoDetector::heap_bytes(size),
    };
    Ok(detector + PASS_MASKS * BitBuffer2::heap_bytes(size))
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
