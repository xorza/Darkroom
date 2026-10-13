//! [`Session`]: one test's measurements, the workers its runs take, and the scratch it writes.

use std::sync::OnceLock;

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

/// One pool per worker count, built and warmed once for the process. The heap's count is the
/// process's, and a pool built or dropped per session starts and ends its threads on their own
/// time: on a busy machine a worker's start-up, or a dropped pool's teardown, lands inside the
/// next measurement.
static POOLS: [OnceLock<ThreadPool>; WORKERS + 1] = [const { OnceLock::new() }; WORKERS + 1];

/// Rayon's global pool, started before anything is measured. Any rayon call outside a session's
/// pool starts it on first use, and its threads start on their own time, inside a measurement;
/// started here, a call that strays onto it finds them running.
static GLOBAL_POOL: OnceLock<()> = OnceLock::new();

/// One test's measurements of [`HEAP`], which [`alone`](crate::alone::alone) leaves its own: its
/// runs work on a pool of their own, and what they write goes under its scratch.
#[derive(Debug)]
pub(crate) struct Session {
    pool: &'static ThreadPool,
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
    /// What the process builds once on first use is built before anything is measured: rayon's
    /// [global pool](GLOBAL_POOL), the resampling kernel's table, which it keeps for its life from
    /// whichever run warps first, and the pool of `workers` workers, at most [`WORKERS`].
    pub(crate) fn on_workers(tag: &str, workers: usize) -> Self {
        GLOBAL_POOL.get_or_init(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build_global()
                .expect("nothing used rayon's global pool before the first session");
            rayon::broadcast(|_| ());
        });
        let pool = POOLS[workers].get_or_init(|| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            // `build` returns before the workers have started; each starts here, not inside a
            // measurement.
            pool.broadcast(|_| ());
            pool
        });
        pool.install(|| {
            let image =
                LinearImage::from_planar_channels(ImageDimensions::new((8, 8), 1), [vec![0.0; 64]]);
            WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5)))
                .warp(&image, WarpParams::default());
        });
        Self {
            pool,
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
