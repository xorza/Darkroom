/// What a RAW decode divided its samples by to reach the pipeline's `[0, 1]` domain.
///
/// The counterpart to [`crate::FitsTransferProvenance`] for the sensor path. Both variants of
/// [`crate::TransferProvenance`] carry a `physical_scale` so two frames can be asked the same
/// question — what one normalized unit is worth in the source's own units — without the caller
/// knowing which decoder produced them.
#[derive(Debug, Clone, PartialEq)]
pub struct RawTransferProvenance {
    /// Multiply a decoded sample by this to recover the sensor's ADU above black.
    ///
    /// `maximum − black` as libraw reported them, which is the span
    /// [`crate::io::raw`] normalized by. Read per file: libraw adjusts `color.maximum` per camera
    /// and per ISO, so two RAWs are only commensurate when this matches.
    pub physical_scale: f32,
}
