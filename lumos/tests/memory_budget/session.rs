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

/// Rayon's global pool, built before anything is measured. Any rayon call outside a session's
/// pool starts it on first use, and its threads start on their own time, inside a measurement;
/// started here, a call that strays onto it finds them running.
///
/// A session of one worker works on it, its one worker the test's own thread — see
/// [`Workers::Calling`].
static GLOBAL_POOL: OnceLock<()> = OnceLock::new();

/// The pool of [`WORKERS`] workers, built and warmed once for the process. The heap's count is the
/// process's, and a pool built or dropped per session starts and ends its threads on their own
/// time: on a busy machine a worker's start-up, or a dropped pool's teardown, lands inside the
/// next measurement.
static POOL: OnceLock<ThreadPool> = OnceLock::new();

/// Where a session's runs work.
#[derive(Debug, Clone, Copy)]
enum Workers {
    /// The test's own thread, as the global pool's one worker: a run works on it, and no other
    /// rayon thread is there to fall asleep. An idle worker sleeps on a `Condvar`, whose pthread
    /// condition std boxes on its first wait where it has no futex — macOS among them — and falls
    /// asleep at its own time, inside a measurement as like as not.
    Calling,
    /// [`POOL`], which the test's thread hands each run to.
    Pool(&'static ThreadPool),
}

impl Workers {
    /// Run `work` on these workers.
    fn run<T: Send>(self, work: impl FnOnce() -> T + Send) -> T {
        match self {
            Self::Calling => work(),
            Self::Pool(pool) => pool.install(work),
        }
    }
}

/// One test's measurements of [`HEAP`], which [`alone`](crate::alone::alone) leaves its own: its
/// runs work on [`Workers`] of their own, and what they write goes under its scratch.
#[derive(Debug)]
pub(crate) struct Session {
    workers: Workers,
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

    /// A session whose runs take one worker, or [`WORKERS`].
    ///
    /// What the process builds once on first use is built before anything is measured: rayon's
    /// [global pool](GLOBAL_POOL), on the test's thread for a session of one worker, the
    /// [pool](POOL) of more, each of its workers started, and the resampling kernel's table, which
    /// it keeps for its life from whichever run warps first. [`alone`](crate::alone::alone) runs
    /// each test in a process of its own, so the first session is the process's.
    pub(crate) fn on_workers(tag: &str, workers: usize) -> Self {
        assert!(
            workers == 1 || workers == WORKERS,
            "a session takes one worker or {WORKERS}, not {workers}"
        );
        GLOBAL_POOL.get_or_init(|| {
            let global = rayon::ThreadPoolBuilder::new().num_threads(1);
            let global = if workers == 1 {
                global.use_current_thread()
            } else {
                global
            };
            global
                .build_global()
                .expect("nothing used rayon's global pool before the first session");
            rayon::broadcast(|_| ());
        });
        let workers = if workers == 1 {
            assert_eq!(
                rayon::current_thread_index(),
                Some(0),
                "the test's thread is the global pool's worker"
            );
            Workers::Calling
        } else {
            Workers::Pool(POOL.get_or_init(|| {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(WORKERS)
                    .build()
                    .unwrap();
                // `build` returns before the workers have started; each starts here, not inside
                // a measurement.
                pool.broadcast(|_| ());
                pool
            }))
        };
        workers.run(|| {
            let image =
                LinearImage::from_planar_channels(ImageDimensions::new((8, 8), 1), [vec![0.0; 64]]);
            WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5)))
                .warp(&image, WarpParams::default());
        });
        Self {
            workers,
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
        let value = self.workers.run(work);
        Measured {
            value,
            peak_bytes: HEAP.peak_bytes() - before,
            kept_bytes: HEAP.live_bytes() as isize - before as isize,
        }
    }
}
