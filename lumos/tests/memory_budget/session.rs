//! [`Session`]: one test's hold on the heap, the workers its runs take, and the scratch it writes.

use std::sync::MutexGuard;

use common::TempDir;
use glam::DVec2;
use lumos::{
    FitsFloatScale, ImageDimensions, IngestConfig, LinearImage, Transform, WarpParams,
    WarpTransform,
};
use rayon::ThreadPool;

use crate::HEAP;
use crate::budget::Budget;

/// The workers a run plans for and works on: few, so that the floors of a stage's smallest step,
/// which grow with the workers, fit below a budget the frames do not, and the same on every
/// machine.
const WORKERS: usize = 2;

/// One test's hold on [`HEAP`], for as long as it lives: the counts are its alone, its runs work
/// on a pool of its own, and what they write goes under its scratch.
#[derive(Debug)]
pub(crate) struct Session {
    _heap: MutexGuard<'static, ()>,
    pool: ThreadPool,
    scratch: TempDir,
}

/// What a closure returned, and the heap it held: the most above what was live as it started,
/// and what it left allocated when it returned.
#[derive(Debug)]
pub(crate) struct Measured<T> {
    pub(crate) value: T,
    pub(crate) peak_bytes: usize,
    pub(crate) kept_bytes: isize,
}

impl Session {
    /// A session for the test `tag` names, its runs on [`WORKERS`] workers.
    pub(crate) fn new(tag: &str) -> Self {
        Self::on_workers(tag, WORKERS)
    }

    /// A session whose runs take `workers` workers.
    ///
    /// What the process builds once on first use is built before anything is measured: the
    /// resampling kernel's table, which it keeps for its life from whichever run warps first.
    pub(crate) fn on_workers(tag: &str, workers: usize) -> Self {
        let heap = HEAP.exclusive();
        let image =
            LinearImage::from_planar_channels(ImageDimensions::new((8, 8), 1), [vec![0.0; 64]]);
        lumos::warp(
            &image,
            &WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5))),
            WarpParams::default(),
        );
        Self {
            _heap: heap,
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap(),
            scratch: TempDir::new(tag),
        }
    }

    pub(crate) const fn scratch(&self) -> &TempDir {
        &self.scratch
    }

    /// How a run reads the session's FITS exposures and plans against `budget`, with its cache
    /// under the scratch.
    pub(crate) fn ingest(&self, budget: Budget) -> IngestConfig {
        let mut ingest = IngestConfig::with_cache_dir(self.scratch.join("cache"));
        ingest.memory_override = Some(budget.memory_override());
        ingest.fits.float_scale = FitsFloatScale::Normalized;
        ingest
    }

    /// Run `work` on the session's workers, and the heap it held.
    pub(crate) fn measure<T: Send>(&self, work: impl FnOnce() -> T + Send) -> Measured<T> {
        let before = HEAP.restart_peak();
        let value = self.pool.install(work);
        Measured {
            value,
            peak_bytes: HEAP.peak_bytes() - before,
            kept_bytes: HEAP.live_bytes() as isize - before as isize,
        }
    }
}
