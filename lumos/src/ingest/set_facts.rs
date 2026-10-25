//! [`SetFacts`]: what every frame of a stack has to state alike.

use crate::frame::frame_facts::FrameFacts;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::metadata::image_provenance::RowOrder;
use crate::image::metadata::sample_domain::SampleDomain;
use crate::ingest::error::AdmissionError;

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
#[derive(Debug)]
pub(crate) struct SetFacts {
    scale: Option<Stated<SampleDomain>>,
    unit: Option<Stated<SampleDomain>>,
    row_order: Option<Stated<RowOrder>>,
    /// The sensor pattern every frame states, `None` for frames no sensor pattern is under: the
    /// first frame's.
    cfa_type: Stated<Option<CfaType>>,
}

/// A fact and the frame that stated it first.
#[derive(Debug)]
struct Stated<T> {
    idx: usize,
    value: T,
}

impl SetFacts {
    /// The facts frame 0 states, which every later frame is held to: a set has facts once it
    /// has a frame.
    pub(super) fn of_first(facts: &FrameFacts) -> Self {
        let mut set = Self {
            scale: None,
            unit: None,
            row_order: None,
            cfa_type: Stated {
                idx: 0,
                value: facts.cfa_type,
            },
        };
        set.record(0, facts);
        set
    }

    /// Check a whole set's facts, frame by frame in index order, each against the facts the
    /// frames before it stated: the frames agree, or the first that does not is named.
    ///
    /// # Panics
    ///
    /// When the set is empty.
    pub(crate) fn check_set<'f>(
        facts: impl IntoIterator<Item = &'f FrameFacts>,
    ) -> Result<(), AdmissionError> {
        let mut facts = facts.into_iter().enumerate();
        let (_, first) = facts.next().expect("a set has a frame");
        let mut set = Self::of_first(first);
        facts.try_for_each(|(idx, facts)| set.admit(idx, facts))
    }

    /// Check frame `idx` against the facts stated before it, then record the ones it states
    /// first. The caller admits frames in index order, so the frame each fact is held to is the
    /// lowest index that states it.
    fn admit(&mut self, idx: usize, facts: &FrameFacts) -> Result<(), AdmissionError> {
        self.check(idx, facts)?;
        self.record(idx, facts);
        Ok(())
    }

    /// Check frame `idx` against the facts recorded so far, without recording its own.
    pub(super) fn check(&self, idx: usize, facts: &FrameFacts) -> Result<(), AdmissionError> {
        if let Some(domain) = &facts.domain {
            let mismatch =
                |reference: &Stated<SampleDomain>| AdmissionError::SampleDomainMismatch {
                    idx,
                    actual: Box::new(domain.clone()),
                    reference_idx: reference.idx,
                    expected: Box::new(reference.value.clone()),
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
            return Err(AdmissionError::RowOrderMismatch {
                idx,
                actual,
                reference_idx: reference.idx,
                expected: reference.value,
            });
        }
        if facts.cfa_type != self.cfa_type.value {
            return Err(AdmissionError::CfaPatternMismatch {
                idx,
                actual: facts.cfa_type,
                reference_idx: self.cfa_type.idx,
                expected: self.cfa_type.value,
            });
        }
        Ok(())
    }

    fn record(&mut self, idx: usize, facts: &FrameFacts) {
        if let Some(domain) = &facts.domain {
            self.scale.get_or_insert_with(|| Stated {
                idx,
                value: domain.clone(),
            });
            if domain.unit.is_some() {
                self.unit.get_or_insert_with(|| Stated {
                    idx,
                    value: domain.clone(),
                });
            }
        }
        if let Some(row_order) = facts.row_order {
            self.row_order.get_or_insert(Stated {
                idx,
                value: row_order,
            });
        }
    }
}
