//! [`SkyNoise`]: what detection keeps of the sky once it is subtracted.

use crate::bg_mesh::BgMesh;
use crate::bg_mesh::mesh_spline::MeshSpline;
use crate::bg_mesh::mesh_surface::MeshSurface;
use crate::bg_mesh::tile_stats::TileComponent;
use crate::image::masked_plane::MaskedPlane;
use crate::math::statistics;
use crate::scratch_pool::{Pooled, ScratchPool};
use crate::star_detection::bg::workspace::BgWorkspace;
use crate::star_detection::star_detection_config::bg_config::BgConfig;

/// A per-pixel σ is floored at this fraction of the frame's typical tile σ: far enough below the
/// real noise never to bind on a healthy estimate, far enough above zero to keep a degenerate tile
/// from collapsing the threshold onto the sky.
const NOISE_FLOOR_FRACTION: f32 = 1e-4;

/// The sky's noise per pixel, read a row or a pixel at a time, and its floor: all the stages after
/// the sky's removal need of it.
///
/// The stages after the removal read the residual plane `pixels − bg`, so the threshold, the
/// labeling, both deblenders and the measurement never see the sky level.
#[derive(Debug)]
pub(crate) struct SkyNoise {
    /// Per-pixel noise (σ).
    pub(crate) noise: Pooled<MeshSurface>,
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
        config: &BgConfig,
        surfaces: &ScratchPool<MeshSurface>,
        workspace: &mut BgWorkspace,
    ) -> Self {
        let MaskedPlane { pixels, mask } = plane;
        let mesh = workspace
            .mesh
            .measure(pixels, mask, None, config.mesh_params());
        Self::of_mesh(
            mesh,
            surfaces,
            MeshReaders {
                spline: &mut workspace.spline,
                values: &mut workspace.values,
            },
        )
    }

    /// The σ surface of `mesh`, from `surfaces`, and its floor.
    pub(super) fn of_mesh(
        mesh: &BgMesh,
        surfaces: &ScratchPool<MeshSurface>,
        readers: MeshReaders<'_>,
    ) -> Self {
        let MeshReaders { spline, values } = readers;
        let floor = noise_floor_from(mesh, values);
        let mut noise = surfaces.acquire();
        spline.solve(mesh, TileComponent::Sigma, &mut noise);
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
/// The spline can carry σ to zero wherever a tile came out flat, which would collapse
/// `bg + σ·noise` onto `bg` and match every pixel above the sky. A fixed constant cannot guard
/// that: the decoder's span sets the magnitude, and a 32-bit integer FITS has its whole noise range
/// near `1e-8`. A fraction of the frame's own median tile σ scales with whatever it is handed.
///
/// With no tile of measurable spread — a synthetic or wholly saturated frame — the sky level stands
/// in, since the interpolated background does not reproduce a constant sky exactly and without a
/// margin above that jitter the whole frame labels as one component.
fn noise_floor_from(grid: &BgMesh, values: &mut Vec<f32>) -> f32 {
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
