//! [`Session`]: one test's measurements, the workers its runs take, and the scratch it writes.

use std::sync::OnceLock;

use common::TempDir;
use glam::DVec2;
use imaginarium::{ChannelLayout, PlanarDesc};
use lumos::{FitsFloatScale, IngestConfig, LinearImage, Transform, WarpConfig, WarpTransform};

use crate::HEAP;
use crate::alone;
use crate::budget::Budget;

/// The workers a run plans for and works on: few, so that the floors of a stage's smallest step,
/// which grow with the workers, fit below a budget the frames do not, and the same on every
/// machine.
const WORKERS: usize = 2;

/// Rayon's global pool, the one pool a process's runs work on — [`alone`](crate::alone::alone)
/// runs each test, and so each session, in a process of its own. Built before anything is
/// measured, on the first session's worker count, with the test's own thread as its first worker:
/// a rayon call finds its threads running rather than starting them inside a measurement, and its
/// jobs take that worker's own queue, where the injector rayon takes jobs from other threads by
/// would allocate a block in whichever measurement its count fills one. [`alone`](crate::alone::alone)
/// marks the test's body as top-level code there (`WorkerPool::top_level`), as `common::par`
/// holds top-level code to starting loops off a worker.
static POOL: OnceLock<usize> = OnceLock::new();

/// One test's measurements of [`HEAP`], which [`alone`](crate::alone::alone) leaves its own: its
/// runs work on the [pool](POOL), and what they write goes under its scratch.
#[derive(Debug)]
pub(crate) struct Session {
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

    /// A session whose runs take `workers` workers, the first the test's own thread.
    ///
    /// What the process builds once on first use is built before anything is measured: the
    /// [pool](POOL), each of its workers started, and the resampling kernel's table, which it keeps
    /// for its life from whichever run warps first.
    ///
    /// Past one worker, a worker waiting on another may first fall asleep inside a measurement: an
    /// idle rayon worker sleeps on a `Condvar`, whose pthread condition std boxes on its first wait
    /// where it has no futex — macOS among them. A budget leaves room for those bytes; a count that
    /// must be exact takes one worker.
    #[expect(
        clippy::disallowed_methods,
        reason = "the harness builds rayon's global pool on its own thread and starts its workers, \
                  which `common::par` leaves to whoever owns the pool"
    )]
    pub(crate) fn on_workers(tag: &str, workers: usize) -> Self {
        assert!(
            alone::in_child(),
            "a session measures inside `alone`, where no other test's threads allocate"
        );
        let built = *POOL.get_or_init(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .use_current_thread()
                .build_global()
                .expect("nothing used rayon's global pool before the first session");
            // `build_global` returns before the other workers have started; each starts here, not
            // inside a measurement.
            rayon::broadcast(|_| ());
            workers
        });
        assert_eq!(built, workers, "one worker count for a process's sessions");
        assert_eq!(
            rayon::current_thread_index(),
            Some(0),
            "a session runs on the thread that built the pool"
        );
        let image = LinearImage::from_planar_channels(
            PlanarDesc::new((8, 8), ChannelLayout::Mono),
            [vec![0.0; 64]],
        );
        WarpConfig::default().warp(
            &image,
            &WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5))),
        );
        Self {
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
    pub(crate) fn measure<T>(&self, work: impl FnOnce() -> T) -> Measured<T> {
        assert!(
            HEAP.counts_this_thread(),
            "a run is measured on a counted thread, not the harness's"
        );
        let before = HEAP.restart_peak();
        let value = work();
        Measured {
            value,
            peak_bytes: usize::try_from(HEAP.peak_bytes() - before)
                .expect("the peak is at least what was live as it restarted"),
            kept_bytes: HEAP.live_bytes() - before,
        }
    }
}
