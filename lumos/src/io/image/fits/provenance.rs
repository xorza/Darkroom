#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitsHduProvenance {
    pub index: usize,
    pub extname: Option<String>,
    pub extver: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitsChecksumState {
    NotChecked,
    Absent,
    Unknown,
    Valid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FitsChecksumProvenance {
    pub datasum: FitsChecksumState,
    pub checksum: FitsChecksumState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FitsTransferProvenance {
    pub bscale: f64,
    pub bzero: f64,
    /// Multiply a decoded sample by this to recover the physical value `BSCALE`/`BZERO` declared.
    ///
    /// The span the decoder divided by to reach `[0, 1]`: `|BSCALE| × (2^bits − 1)` for an integer
    /// `BITPIX`, and for a floating-point one either the 16-bit full scale (when `DATAMAX` declared
    /// a saturation level well above unity, so the samples were ADU) or `1.0` (when it did not).
    pub physical_scale: f32,
    pub unit: Option<String>,
    pub hdu: FitsHduProvenance,
    pub checksum: FitsChecksumProvenance,
}
