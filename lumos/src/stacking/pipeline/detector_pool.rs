//! Bounded detector reuse for frame-parallel pipeline stages.

use crate::concurrency;
use crate::error::InvalidConfigField;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::StarDetector;

/// One [`StarDetector`] per concurrency slot, reused across batches so each batch inherits the
/// previous one's warmed buffer pools.
///
/// Deliberately not a [`JobScratchPool`](crate::concurrency::JobScratchPool), and the type system
/// will not stop you: `StarDetector` implements `Default`, so `JobScratchPool<StarDetector>`
/// compiles — and then quietly hands out detectors built from `Config::default()` instead of the
/// caller's detection config, because the pool fills gaps with `T::default()`. Building every
/// slot from `config` up front is also what makes an invalid configuration fail here rather than
/// from inside the parallel closure.
#[derive(Debug)]
pub(crate) struct DetectorPool {
    detectors: Vec<StarDetector>,
}

impl DetectorPool {
    /// Build `max_concurrent` detectors from one configuration, rejecting an invalid one before
    /// any frame is touched.
    pub(crate) fn from_config(
        config: &Config,
        max_concurrent: usize,
    ) -> Result<Self, InvalidConfigField> {
        assert!(max_concurrent > 0, "max_concurrent must be > 0");
        let detectors = (0..max_concurrent)
            .map(|_| StarDetector::from_config(config.clone()))
            .collect::<Result<_, _>>()?;
        Ok(Self { detectors })
    }

    /// Map `f` over `items` with one detector per concurrent slot, passing each item's index.
    ///
    /// The detectors *are* the slots of [`concurrency::try_par_map_bounded`], so concurrency is
    /// capped at the pool size and each detector carries its warmed buffers from one frame to
    /// the next it happens to pick up. Which frames a given detector sees is not fixed: a
    /// detector takes whatever is next when it frees up.
    pub(crate) fn try_map<T, R, E, F>(&mut self, items: &[T], f: F) -> Result<Vec<R>, E>
    where
        T: Sync,
        R: Send,
        E: Send,
        F: Fn(&mut StarDetector, usize, &T) -> Result<R, E> + Sync,
    {
        concurrency::try_par_map_bounded(items.len(), &mut self.detectors, |detector, index| {
            f(detector, index, &items[index])
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use parking_lot::Mutex;

    use crate::stacking::pipeline::detector_pool::DetectorPool;
    use crate::stacking::star_detection::config::Config;
    use crate::stacking::star_detection::detector::StarDetector;

    #[derive(Debug, PartialEq, Eq)]
    struct DetectorUse {
        item: usize,
        detector_address: usize,
    }

    #[test]
    fn results_stay_ordered_and_never_use_more_detectors_than_slots() {
        let mut pool = DetectorPool::from_config(&Config::default(), 2).unwrap();
        let uses = pool
            .try_map(&[0, 1, 2, 3, 4], |detector, _index, &item| {
                Ok::<_, ()>(DetectorUse {
                    item,
                    detector_address: (detector as *const StarDetector).addr(),
                })
            })
            .unwrap();

        assert_eq!(
            uses.iter().map(|usage| usage.item).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
        // Which detector sees which frame is up to whichever frees up first; what the pool
        // guarantees is that five frames never conjure more than two detectors.
        let distinct: HashSet<usize> = uses.iter().map(|usage| usage.detector_address).collect();
        assert!(
            distinct.len() <= 2,
            "five items used {} detectors from a 2-slot pool",
            distinct.len()
        );
    }

    #[test]
    fn an_error_stops_the_pool_taking_further_items() {
        let mut pool = DetectorPool::from_config(&Config::default(), 2).unwrap();
        let items: Vec<usize> = (0..1000).collect();

        let attempted = Mutex::new(Vec::new());
        let error = pool
            .try_map(&items, |_, _index, &item| {
                attempted.lock().push(item);
                if item == 0 { Err(item) } else { Ok(item) }
            })
            .unwrap_err();

        assert_eq!(error, 0);
        let ran = attempted.into_inner().len();
        assert!(ran <= 16, "ran {ran} of 1000 after an immediate failure");
    }
}
