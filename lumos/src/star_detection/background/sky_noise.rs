//! [`SkyNoise`]: what detection keeps of the sky once it is subtracted.

use imaginarium::Buffer2;

use crate::background_mesh::BackgroundMesh;
use crate::background_mesh::mesh_spline::MeshSpline;
use crate::background_mesh::tile_stats::TileComponent;
use crate::image::masked_plane::MaskedPlane;
use crate::math::statistics;
use crate::scratch_pool::{Pooled, ScratchPool};
use crate::star_detection::background::workspace::BackgroundWorkspace;
use crate::star_detection::star_detection_config::background_config::BackgroundConfig;

/// A per-pixel σ is floored at this fraction of the frame's typical tile σ: far enough below the
/// real noise never to bind on a healthy estimate, far enough above zero to keep a degenerate tile
/// from collapsing the threshold onto the sky.
const NOISE_FLOOR_FRACTION: f32 = 1e-4;

/// The sky's noise per pixel and its floor: all the stages after the sky's removal need of it.
///
/// The sky level itself went into the residual plane `pixels − background` that those stages read,
/// so none of them can see sky-included values — the threshold, the labeling, both deblenders and
/// the measurement all work on the source alone.
#[derive(Debug)]
pub(crate) struct SkyNoise {
    /// Per-pixel noise (σ).
    pub(crate) noise: Pooled<Buffer2<f32>>,
    /// The floor every threshold built from [`Self::noise`] applies to it — see
    /// [`noise_floor_from`].
    pub(crate) floor: f32,
}

impl SkyNoise {
    /// The noise of `plane` alone, by the background estimate's mesh, leaving out the pixels its
    /// mask sets: for a plane whose sky is already out, whose σ is all a threshold needs. Only
    /// the σ spline runs.
    pub(crate) fn measure(
        plane: MaskedPlane<'_>,
        config: &BackgroundConfig,
        planes: &ScratchPool<Buffer2<f32>>,
        workspace: &mut BackgroundWorkspace,
    ) -> Self {
        let MaskedPlane { pixels, mask } = plane;
        let mesh = workspace
            .mesh
            .measure(pixels, mask, None, config.mesh_params());
        Self::of_mesh(
            mesh,
            planes,
            MeshReaders {
                spline: &mut workspace.spline,
                values: &mut workspace.values,
            },
        )
    }

    /// The σ plane of `mesh`, from `planes`, and its floor.
    pub(super) fn of_mesh(
        mesh: &BackgroundMesh,
        planes: &ScratchPool<Buffer2<f32>>,
        readers: MeshReaders<'_>,
    ) -> Self {
        let MeshReaders { spline, values } = readers;
        let floor = noise_floor_from(mesh, values);
        let mut noise = planes.acquire();
        spline.interpolate(mesh, TileComponent::Sigma, &mut noise);
        Self { noise, floor }
    }
}

/// What reads a measured mesh out to a frame: its spline, and the buffer the noise floor's median
/// sorts.
#[derive(Debug)]
pub(super) struct MeshReaders<'a> {
    pub(super) spline: &'a mut MeshSpline,
    pub(super) values: &'a mut Vec<f32>,
}

/// The per-pixel σ floor for a frame whose typical noise is `scale`.
///
/// Shared so the estimator and the synthetic uniform maps the tests build cannot state the fraction
/// differently.
pub(crate) fn noise_floor_for(scale: f32) -> f32 {
    (scale * NOISE_FLOOR_FRACTION).max(f32::MIN_POSITIVE)
}

/// The frame's own floor for a per-pixel noise estimate.
///
/// The spline interpolates σ between tile centers and can carry it to zero wherever a tile came out
/// flat, which would collapse `bg + σ·noise` onto `bg` and match every pixel above the sky. A fixed
/// constant cannot be that guard: the linear domain is `[0, 1]`, but the span the decoder divided
/// by sets the magnitude, so one frame's entire noise range can sit below a constant sized for
/// another — a 32-bit integer FITS lands near `1e-8` — and the threshold then rejects everything
/// instead. A fraction of this frame's own median tile σ scales with whatever it is handed.
///
/// When no tile has a measurable spread at all — a synthetic or wholly saturated frame — the sky
/// level stands in for it. The floor still has work to do there: the interpolated background map
/// does not reproduce a constant sky exactly, and without a margin above that jitter every pixel
/// clears `bg` and the whole frame labels as one component. The sky carries the frame's magnitude,
/// so a fraction of it stays above the jitter in any domain.
fn noise_floor_from(grid: &BackgroundMesh, values: &mut Vec<f32>) -> f32 {
    let stats = grid.stats.pixels();
    values.clear();
    values.extend(
        stats
            .iter()
            .map(|tile| tile.sigma)
            .filter(|sigma| sigma.is_finite() && *sigma > 0.0),
    );
    if values.is_empty() {
        values.extend(
            stats
                .iter()
                .map(|tile| tile.sky.abs())
                .filter(|sky| sky.is_finite() && *sky > 0.0),
        );
        if values.is_empty() {
            return f32::MIN_POSITIVE;
        }
    }
    noise_floor_for(statistics::median_mut(values))
}
