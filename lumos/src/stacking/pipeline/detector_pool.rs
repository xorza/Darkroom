//! Bounded detector reuse for frame-parallel pipeline stages.

use rayon::prelude::*;

use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::StarDetector;
use crate::stacking::star_detection::error::StarDetectionConfigError;

#[derive(Debug)]
pub(crate) struct DetectorPool {
    detectors: Vec<StarDetector>,
}

impl DetectorPool {
    pub(crate) fn from_config(
        config: &Config,
        max_concurrent: usize,
    ) -> Result<Self, StarDetectionConfigError> {
        assert!(max_concurrent > 0, "max_concurrent must be > 0");
        let detectors = (0..max_concurrent)
            .map(|_| StarDetector::from_config(config.clone()))
            .collect::<Result<_, _>>()?;
        Ok(Self { detectors })
    }

    pub(crate) fn try_map<T, R, E, F>(&mut self, items: &[T], f: F) -> Result<Vec<R>, E>
    where
        T: Sync,
        R: Send,
        E: Send,
        F: Fn(&mut StarDetector, &T) -> Result<R, E> + Sync,
    {
        let mut results = Vec::with_capacity(items.len());
        for chunk in items.chunks(self.detectors.len()) {
            let chunk_results: Result<Vec<R>, E> = self
                .detectors
                .par_iter_mut()
                .zip(chunk.par_iter())
                .map(|(detector, item)| f(detector, item))
                .collect();
            results.extend(chunk_results?);
        }
        Ok(results)
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
            .try_map(&[0, 1, 2, 3, 4], |detector, &item| {
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
            .try_map(&[0, 1, 2, 3, 4], |_, &item| {
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
