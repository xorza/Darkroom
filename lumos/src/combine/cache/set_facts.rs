//! [`SetFacts`]: what every frame of a stack has to state alike.

use crate::combine::error::Error;
use crate::frame_store::frame_facts::FrameFacts;
use crate::io::image::cfa::CfaType;
use crate::io::image::image_provenance::RowOrder;
use crate::io::image::sample_domain::SampleDomain;

/// The facts a frame set must agree on — sample domain, row order and CFA pattern — each held by
/// the first frame that states it, and checked in that order.
///
/// A frame that states no domain or row order — synthesized rather than decoded — is skipped for
/// that fact rather than taken as agreeing: there is nothing to compare, and refusing on it would
/// refuse every in-memory fixture. The CFA pattern every frame states, `None` being "no mosaic".
///
/// Scale and unit get a reference each, because a frame can state a scale and no unit, and
/// [`SampleDomain::units_agree`] is blind across that gap, so it is not transitive. Comparing
/// everything with frame 0 alone would let a `Jy/beam` frame and a `count/s` frame through
/// whenever frame 0 stated no unit; here the first frame to state a unit owns that half.
#[derive(Debug, Default)]
pub(crate) struct SetFacts {
    scale: Option<Stated<SampleDomain>>,
    unit: Option<Stated<SampleDomain>>,
    row_order: Option<Stated<RowOrder>>,
    cfa_type: Option<Stated<Option<CfaType>>>,
}

/// A fact and the frame that stated it first.
#[derive(Debug)]
struct Stated<T> {
    index: usize,
    value: T,
}

impl SetFacts {
    /// The facts frame 0 states, for a frame decoding beside the others to check against before
    /// the set is complete, so a mismatched set stops before the rest decodes.
    pub(crate) fn of_first(facts: &FrameFacts) -> Self {
        let mut set = Self::default();
        set.record(0, facts);
        set
    }

    /// Check frame `index` against the facts stated before it, then record the ones it states
    /// first. Frames are admitted in index order, which is what makes the reference the first.
    pub(crate) fn admit(&mut self, index: usize, facts: &FrameFacts) -> Result<(), Error> {
        self.check(index, facts)?;
        self.record(index, facts);
        Ok(())
    }

    /// Check frame `index` against the facts recorded so far, without recording its own.
    pub(crate) fn check(&self, index: usize, facts: &FrameFacts) -> Result<(), Error> {
        if let Some(domain) = &facts.domain {
            let mismatch = |reference: &Stated<SampleDomain>| Error::SampleDomainMismatch {
                index,
                actual: domain.clone(),
                reference_index: reference.index,
                expected: reference.value.clone(),
            };
            if let Some(scale) = &self.scale
                && domain.conversion_to(&scale.value).is_none()
            {
                return Err(mismatch(scale));
            }
            if domain.unit.is_some()
                && let Some(unit) = &self.unit
                && domain.unit != unit.value.unit
            {
                return Err(mismatch(unit));
            }
        }
        if let (Some(actual), Some(reference)) = (facts.row_order, &self.row_order)
            && actual != reference.value
        {
            return Err(Error::RowOrderMismatch {
                index,
                actual,
                reference_index: reference.index,
                expected: reference.value,
            });
        }
        if let Some(reference) = &self.cfa_type
            && facts.cfa_type != reference.value
        {
            return Err(Error::CfaPatternMismatch {
                index,
                actual: facts.cfa_type,
                reference_index: reference.index,
                expected: reference.value,
            });
        }
        Ok(())
    }

    fn record(&mut self, index: usize, facts: &FrameFacts) {
        if let Some(domain) = &facts.domain {
            self.scale.get_or_insert_with(|| Stated {
                index,
                value: domain.clone(),
            });
            if domain.unit.is_some() {
                self.unit.get_or_insert_with(|| Stated {
                    index,
                    value: domain.clone(),
                });
            }
        }
        if let Some(row_order) = facts.row_order {
            self.row_order.get_or_insert(Stated {
                index,
                value: row_order,
            });
        }
        self.cfa_type.get_or_insert(Stated {
            index,
            value: facts.cfa_type,
        });
    }
}
