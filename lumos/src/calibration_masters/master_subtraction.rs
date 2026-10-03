//! [`MasterSubtraction`]: a master taken from every frame of a calibration stack.

use crate::combine::error::Error;
use crate::ingest::frame_step::FrameStep;
use crate::io::image::cfa::CfaImage;
use crate::io::image::sample_domain::DomainMap;

/// A master taken from each frame of a stack before its statistics and the combine: a flat's
/// flat-dark or bias, so the multiplicative normalization scales the flat's own signal.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MasterSubtraction<'a> {
    pub(crate) master: &'a CfaImage,
}

impl FrameStep<CfaImage> for MasterSubtraction<'_> {
    fn apply(&self, index: usize, frame: &mut CfaImage) -> Result<(), Error> {
        let master = self.master;
        if master.cfa_type != frame.cfa_type || master.size() != frame.size() {
            return Err(Error::SubtractorShape {
                index,
                frame: frame.size(),
                subtractor: master.size(),
            });
        }
        let map = match (&frame.metadata.domain, &master.metadata.domain) {
            (Some(frame_domain), Some(master_domain)) => master_domain
                .conversion_to(frame_domain)
                .ok_or_else(|| Error::SubtractorDomain {
                    index,
                    frame: Box::new(frame_domain.clone()),
                    subtractor: Box::new(master_domain.clone()),
                })?,
            _ => DomainMap::IDENTITY,
        };
        frame.subtract(master, map);
        frame.metadata.calibrated = true;
        Ok(())
    }
}
