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

impl FitsChecksumProvenance {
    /// Neither keyword checked: what a load under [`FitsChecksumPolicy::Ignore`] records.
    ///
    /// [`FitsChecksumPolicy::Ignore`]: crate::FitsChecksumPolicy::Ignore
    pub(crate) const NOT_CHECKED: Self = Self {
        datasum: FitsChecksumState::NotChecked,
        checksum: FitsChecksumState::NotChecked,
    };
}

#[derive(Debug, Clone, PartialEq)]
pub struct FitsTransferProvenance {
    pub bscale: f64,
    pub bzero: f64,
    pub hdu: FitsHduProvenance,
    pub checksum: FitsChecksumProvenance,
}
