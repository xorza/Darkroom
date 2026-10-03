//! L.A.Cosmic where no dense same-colour sub-lattice exists.
//!
//! X-Trans has no 2×2 phase to deinterleave, so detection runs on the mosaic itself with
//! same-colour stencils gathered through [`CfaType::color_at`]. Median-based, so a cosmic ray
//! inside a stencil cannot drag its own reference, and without the ×2 subsample — same-colour
//! sampling is already coarse, and the iteration handles multi-pixel hits.

use rayon::prelude::*;

use crate::background_mesh::colour_mesh::ColourMesh;
use crate::bit_buffer2::BitBuffer2;
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::cfa_lattice::{CfaLattice, Gathered};
use crate::math::size2us::Size2us;
use crate::math::statistics::median_mut;
use crate::math::vec2us::Vec2us;

use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::calibration_masters::cosmic_ray::masks::CrMasks;
use crate::calibration_masters::cosmic_ray::noise_model::NoiseModel;

/// Radius (px) scanned for same-color neighbors — one X-Trans period (6×6) contains every color.
/// Nearest same-color neighbors for the "fine" median; the coarse median uses all gathered.
const XTRANS_SMALL: usize = 8;
/// Cap on gathered same-color neighbors (the coarse median scale).
const XTRANS_LARGE: usize = 24;
/// Nearest unmasked same-color neighbors used to in-paint a flagged pixel.
const XTRANS_REPLACE: usize = 12;

/// Frame-sized `f32` planes the X-Trans detector holds: `lplus`, `f`, `signal`, `noise` and
/// `frame`.
pub(crate) const XTRANS_SCRATCH_PLANES: usize = 5;

/// The CFA detector's per-pixel inputs and the scratch that builds them, allocated on the first
/// iteration and reused by every one after it — [`MonoDetector`](super::mono::MonoDetector)'s rule
/// on the X-Trans path.
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
    /// The read-only snapshot [`xtrans_replace`] gathers from.
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
    noise: NoiseModel,
    /// Same-colour neighbour geometry, built once per detector: recomputing the neighbour set per
    /// pixel — a 13×13 colour sweep plus a distance sort — dominated the scan.
    lattice: CfaLattice,
    scratch: XtransScratch,
}

impl<'a> XtransDetector<'a> {
    pub(super) fn new(config: &'a CosmicRayConfig, noise: NoiseModel, cfa: &'a CfaType) -> Self {
        let CfaType::XTrans(pattern) = cfa else {
            panic!("XtransDetector requires an X-Trans pattern, got {cfa:?}");
        };
        Self {
            cfa,
            config,
            noise,
            lattice: CfaLattice::new(&CfaType::XTrans(*pattern)),
            scratch: XtransScratch::default(),
        }
    }

    /// The bytes a detection on a `size` mosaic allocates beside it: the scratch planes and the
    /// masks, or nothing on a mosaic too small to scan.
    pub(super) fn heap_bytes(size: Size2us) -> usize {
        if size.width < 7 || size.height < 7 {
            return 0;
        }
        XTRANS_SCRATCH_PLANES * size.pixel_count() * size_of::<f32>() + CrMasks::heap_bytes(size)
    }

    /// Detect and in-paint cosmic rays on the mosaic, in place, returning the CR pixel count.
    ///
    /// Median-based, so a ray inside a stencil cannot drag its own reference, and **without** the
    /// mono path's ×2 subsample — same-colour sampling is already coarse and the iteration handles
    /// multi-pixel hits. Significance is `S = L⁺/N` with no `S'` median subtraction, since `L⁺`
    /// (excess over the same-colour median) is already a local high-pass.
    pub(super) fn reject(
        &mut self,
        data: &mut [f32],
        size: Size2us,
        mesh: &ColourMesh,
        found: &mut BitBuffer2,
    ) -> usize {
        debug_assert_eq!(data.len(), size.pixel_count());
        if size.width < 7 || size.height < 7 {
            return 0;
        }
        let mut masks = CrMasks::new(size);
        let scratch = &mut self.scratch;
        // Sized exactly up front, so the working set is the planes `XTRANS_SCRATCH_PLANES` counts.
        scratch.frame.reserve_exact(size.pixel_count());

        for _ in 0..self.config.niter {
            let scene = CfaScene {
                pix: data,
                size,
                cfa: self.cfa,
                mask: &masks.accumulated,
            };
            scratch.fill_structure(&scene, &self.lattice);
            scratch.fill_noise(&scene, mesh, self.noise);
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
                &self.lattice,
                &mut scratch.frame,
            );
        }

        found.copy_from(&masks.accumulated);
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
    /// (one gather per pixel: nearest-`XTRANS_LARGE`, with the nearest-`XTRANS_SMALL` subset, each
    /// with its ties).
    fn fill_structure(&mut self, scene: &CfaScene<'_>, lattice: &CfaLattice) {
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
            .for_each_init(Gathered::default, |gathered, (y, ((lrow, frow), srow))| {
                for x in 0..w {
                    let v = scene.pix[y * w + x];
                    lattice.gather(
                        scene.pix,
                        scene.size,
                        Vec2us::new(x, y),
                        XTRANS_LARGE,
                        |index| !scene.mask.get(index),
                        gathered,
                    );
                    if gathered.values.is_empty() {
                        frow[x] = 0.0;
                        srow[x] = v;
                        continue;
                    }
                    // Nearest-first, so the two scales are prefixes of one gather. The coarse
                    // median reorders the values, so the fine one is taken first.
                    let small = gathered.tie_end(XTRANS_SMALL);
                    let med_small = median_mut(&mut gathered.values[..small]);
                    let med_large = median_mut(&mut gathered.values);
                    lrow[x] = (v - med_small).max(0.0);
                    // Non-negative only — see the mono detector: the σ-unit floor downstream
                    // is what guards the contrast ratio, at any sample scale.
                    frow[x] = (med_small - med_large).max(0.0);
                    srow[x] = med_small;
                }
            });
    }

    /// Per-pixel noise for the CFA path, from the signal estimate [`Self::fill_structure`] left and
    /// the local background of the pixel's own colour: R, G and B sit at different sky levels after
    /// flat-fielding, so one background for the mosaic would inflate σ.
    fn fill_noise(&mut self, scene: &CfaScene<'_>, mesh: &ColourMesh, noise: NoiseModel) {
        let Self {
            signal, noise: out, ..
        } = self;
        let size = scene.size;
        out.resize(size.pixel_count(), 0.0);
        out.par_iter_mut()
            .zip(&*signal)
            .enumerate()
            .for_each(|(index, (out, &signal))| {
                let position = size.point_of(index);
                let colour = usize::from(scene.cfa.color_at(position));
                *out = noise.noise(signal, mesh.at(colour, position));
            });
    }
}

/// Replace masked pixels with the median of their nearest unmasked same-color neighbors. Gathers
/// from a snapshot in the caller's `snapshot` buffer, for the reason
/// [`replace_flagged`](super::mono::replace_flagged) gives.
fn xtrans_replace(
    data: &mut [f32],
    size: Size2us,
    cfa: &CfaType,
    mask: &BitBuffer2,
    lattice: &CfaLattice,
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
    data.par_chunks_mut(w)
        .enumerate()
        .for_each_init(Gathered::default, |gathered, (y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                if !mask[y * w + x] {
                    continue;
                }
                lattice.gather(
                    scene.pix,
                    scene.size,
                    Vec2us::new(x, y),
                    XTRANS_REPLACE,
                    |index| !mask.get(index),
                    gathered,
                );
                if gathered.values.is_empty() {
                    continue;
                }
                *o = median_mut(&mut gathered.values);
            }
        });
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::background_mesh::colour_mesh::ColourMesh;
    use crate::background_mesh::workspace::MeshWorkspace;
    use crate::bit_buffer2::BitBuffer2;
    use crate::calibration_masters::cosmic_ray::config::{CosmicRayConfig, NoiseEstimation};
    use crate::calibration_masters::cosmic_ray::noise_model::NoiseModel;
    use crate::calibration_masters::cosmic_ray::xtrans::{XtransDetector, XtransScratch};
    use crate::io::image::cfa::CfaType;
    use crate::io::image::image_metadata::ImageMetadata;
    use crate::math::size2us::Size2us;

    /// Total capacity, in floats, of the X-Trans detector's working set after a run on `data`.
    /// Destructured so a buffer added to [`XtransScratch`] fails to compile here.
    pub(crate) fn xtrans_scratch_floats(data: &mut [f32], size: Size2us, cfa: &CfaType) -> usize {
        let config = CosmicRayConfig::default();
        let noise = NoiseModel::resolve(&NoiseEstimation::Measured, &ImageMetadata::default())
            .expect("the measured model needs nothing from the frame");
        let mesh = ColourMesh::measure(
            &Buffer2::new(size.width, size.height, data.to_vec()),
            cfa,
            64,
            &mut MeshWorkspace::default(),
        );
        let mut detector = XtransDetector::new(&config, noise, cfa);
        detector.reject(data, size, &mesh, &mut BitBuffer2::new_default(size));
        let XtransScratch {
            lplus,
            f,
            signal,
            noise,
            frame,
        } = &detector.scratch;
        lplus.capacity() + f.capacity() + signal.capacity() + noise.capacity() + frame.capacity()
    }
}
