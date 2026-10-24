//! [`Deblenders`]: the deblending sets of a detector's workers, and the regions they split into.

use std::sync::MutexGuard;

use common::PerWorker;

use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::region::Region;

/// One [`DeblendBuffers`] for each worker, kept from frame to frame, and the regions they split a
/// frame's components into, gathered back into the components' order.
///
/// A set a component was deblended in holds its regions, each beside its component's place, so a
/// frame's working set is one set a worker however rayon splits the components: a set leased for
/// each split would count the splits, which follow the schedule.
#[derive(Debug, Default)]
pub(crate) struct Deblenders {
    sets: PerWorker<DeblendBuffers>,
    /// The regions of every set, each with its component's place and its place in the set.
    gathered: Vec<GatheredRegion>,
}

/// A region a set holds, and where it falls in the components' order.
#[derive(Debug)]
struct GatheredRegion {
    component: usize,
    /// Its place in the set, which keeps a component's regions in the order they were split in.
    seq: usize,
    region: Region,
}

impl Deblenders {
    /// The calling worker's set.
    pub(crate) fn set(&self) -> MutexGuard<'_, DeblendBuffers> {
        self.sets.get()
    }

    /// The regions every set holds, appended to `regions` in their components' order, each
    /// component's in the order it was split into them; the sets keep their room.
    pub(crate) fn gather_into(&mut self, regions: &mut Vec<Region>) {
        let Self { sets, gathered } = self;
        gathered.clear();
        for set in sets.iter_mut() {
            debug_assert_eq!(
                set.regions.len(),
                set.components.len(),
                "a component a region"
            );
            let found = set.regions.drain(..).zip(set.components.drain(..));
            gathered.extend(
                found
                    .enumerate()
                    .map(|(seq, (region, component))| GatheredRegion {
                        component,
                        seq,
                        region,
                    }),
            );
        }
        gathered.sort_unstable_by_key(|found| (found.component, found.seq));
        regions.extend(gathered.drain(..).map(|found| found.region));
    }
}
