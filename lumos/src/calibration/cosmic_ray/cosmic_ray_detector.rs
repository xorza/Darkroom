//! [`CosmicRayDetector`]: single-frame cosmic-ray rejection, its working set kept from one frame
//! to the next.

use crate::bg_mesh::color_mesh::ColorMesh;
use crate::calibration::cosmic_ray::bayer::{BayerDetection, BayerDetector};
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::calibration::cosmic_ray::error::CosmicRayError;
use crate::calibration::cosmic_ray::kept_out::KeptOut;
use crate::calibration::cosmic_ray::mono::Detection;
use crate::calibration::cosmic_ray::noise_model::{NoiseModel, PixelBgSource, PixelBgs};
use crate::calibration::cosmic_ray::{BG_TILE_SIZE, Layout};
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::masked_plane::MaskedPlane;

/// Detects and in-paints cosmic rays in calibrated mosaics, one at a time, keeping its planes and
/// masks from one to the next, so a worker's later frames fault no new pages in.
#[derive(Debug)]
pub(crate) struct CosmicRayDetector {
    /// A mono frame is scanned by its mono detector, a Bayer one phase by phase through it.
    bayer: BayerDetector,
}

impl CosmicRayDetector {
    pub(crate) fn new() -> Self {
        Self {
            bayer: BayerDetector::new(),
        }
    }

    /// Detect and in-paint cosmic rays in `image`, in place, under `config`, dispatching on its CFA
    /// type (mono / Bayer), and flag every in-painted pixel [`QualityFlags::COSMIC_RAY`] and
    /// [`QualityFlags::REPAIRED`]. Saturated star cores and pixels with no measurement are kept
    /// out of detection, the background and every in-paint's sources, as [`KeptOut`] says.
    /// Returns the number of CR pixels corrected, or an error, with the frame untouched, for an
    /// X-Trans mosaic or when the parametric noise model needs an ADC step the frame does not
    /// record.
    pub(crate) fn reject(
        &mut self,
        image: &mut CfaImage,
        config: &CosmicRayConfig,
    ) -> Result<usize, CosmicRayError> {
        let layout = Layout::of(&image.cfa_type())?;
        let noise = NoiseModel::resolve(&config.noise, &image.metadata)?;
        let size = image.data().size();
        let kept_out = KeptOut::of(image.data(), image.flags());
        // Measured once, before any repair: a hit spoils a tile's statistics no more than a star
        // does, and the mesh's clip and median filter are what keep both out.
        let mesh = ColorMesh::measure(
            MaskedPlane {
                pixels: image.data(),
                mask: kept_out.as_ref().map(|kept_out| &kept_out.never_flagged),
            },
            &image.cfa_type(),
            BG_TILE_SIZE,
        );
        let edit = image.data_mut_with_gain();
        let bgs = PixelBgs {
            mesh: &mesh,
            flat_gain: edit.flat_gain.map(|gain| &**gain),
        };
        // Bayer is 2×2-periodic → four dense same-color planes; the mono detector runs per plane.
        let found = match layout {
            Layout::Bayer(pattern) => self.bayer.reject(
                edit.data,
                pattern,
                BayerDetection {
                    config,
                    noise,
                    bgs,
                    kept_out: kept_out.as_ref(),
                },
            ),
            Layout::Mono => self.bayer.mono().reject(
                edit.data,
                Detection {
                    config,
                    noise,
                    local: PixelBgSource::Plane { bgs, size },
                    kept_out: kept_out.as_ref(),
                },
            ),
        };
        let count = found.count_ones();
        if count > 0 {
            image.add_flags_where(
                QualityFlags::COSMIC_RAY.union(QualityFlags::REPAIRED),
                |idx| found.get(idx),
            );
        }
        Ok(count)
    }
}
