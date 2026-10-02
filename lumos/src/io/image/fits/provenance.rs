use crate::io::image::sample_domain::ScaleOrigin;

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
    /// For an integer `BITPIX` it is the span the decoder divided by to reach `[0, 1]`,
    /// `|BSCALE| × (2^bits − 1)`. For a floating-point one it is the full scale the caller gave,
    /// the `LUMSCALE` a lumos-written file recorded (whose samples are already normalized), or the
    /// `FitsFloatScale::Auto` guess from `DATAMAX`.
    pub physical_scale: f32,
    /// Whether [`Self::physical_scale`] was declared or guessed — see
    /// [`ScaleOrigin`](crate::ScaleOrigin).
    pub scale_origin: ScaleOrigin,
    pub unit: Option<String>,
    pub hdu: FitsHduProvenance,
    pub checksum: FitsChecksumProvenance,
}
