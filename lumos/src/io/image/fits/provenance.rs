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
    pub unit: Option<String>,
    pub hdu: FitsHduProvenance,
    pub checksum: FitsChecksumProvenance,
}
