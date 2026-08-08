//! Bounded detector reuse for frame-parallel pipeline stages.

use rayon::prelude::*;

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
    /// The batch width is the slot count, so the window that
    /// [`concurrency::try_collect_batches`] hands back indexes the detectors directly: slot *k*
    /// always takes the *k*-th item of every window, and carries its warmed buffers into the
    /// next one.
    pub(crate) fn try_map<T, R, E, F>(&mut self, items: &[T], f: F) -> Result<Vec<R>, E>
    where
        T: Sync,
        R: Send,
        E: Send,
        F: Fn(&mut StarDetector, usize, &T) -> Result<R, E> + Sync,
    {
        let detectors = &mut self.detectors;
        let slots = detectors.len();
        concurrency::try_collect_batches(items.len(), slots, |batch| {
            detectors[..batch.len()]
                .par_iter_mut()
                .zip(items[batch.start..batch.end].par_iter())
                .enumerate()
                .map(|(offset, (detector, item))| f(detector, batch.start + offset, item))
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
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
    fn slots_are_reused_across_ordered_batches() {
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
        assert_ne!(uses[0].detector_address, uses[1].detector_address);
        assert_eq!(uses[0].detector_address, uses[2].detector_address);
        assert_eq!(uses[1].detector_address, uses[3].detector_address);
        assert_eq!(uses[0].detector_address, uses[4].detector_address);

        let attempted = Mutex::new(Vec::new());
        let error = pool
            .try_map(&[0, 1, 2, 3, 4], |_, _index, &item| {
                attempted.lock().push(item);
                if item == 2 { Err(item) } else { Ok(item) }
            })
            .unwrap_err();
        assert_eq!(error, 2);
        assert!(
            !attempted.into_inner().contains(&4),
            "an error in the second batch must prevent the third batch from starting"
        );
    }
}
