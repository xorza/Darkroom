//! [`FitsTransferProvenance`]: how a FITS image's stored values became its samples, and which
//! HDU they came from.

use serde::{Deserialize, Serialize};

use crate::io::fits::fits_checksum_provenance::FitsChecksumProvenance;

/// How a FITS image's stored values became its samples: the `BSCALE` and `BZERO` applied, the HDU
/// read, and what its checksums said.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FitsTransferProvenance {
    pub bscale: f64,
    pub bzero: f64,
    pub hdu: FitsHduProvenance,
    pub checksum: FitsChecksumProvenance,
}

/// The HDU an image was read from: its position in the file, and the name and version it states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FitsHduProvenance {
    pub index: usize,
    pub extname: Option<String>,
    pub extver: Option<i64>,
}
