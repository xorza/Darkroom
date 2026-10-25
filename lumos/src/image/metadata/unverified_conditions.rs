//! [`UnverifiedConditions`]: the capture conditions a dark match could not compare.

/// The capture conditions not compared when a master holding dark signal was taken from a frame:
/// the frame or the master did not state them. A dark of another exposure or temperature leaves a
/// residual of dark signal that nothing reports, so the record says where the match was not
/// checked; a temperature that was compared and differed is the
/// [`DarkComparison`](crate::DarkComparison)'s difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnverifiedConditions {
    /// The exposures were not compared.
    pub exposure: bool,
    /// No temperature was compared: the two did not both state the sensor's, nor the camera
    /// body's — most DSLRs state neither, Canon only the body's.
    pub temperature: bool,
}
