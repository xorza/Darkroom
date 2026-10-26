//! Why a calibration bundle could not be written or read.

use common::Size2us;
use std::io;

use crate::calibration::error::CalibrationError;
use crate::calibration::master_role::MasterRole;
use crate::io::error::WriteError;
use crate::io::fits::error::FitsLoadError;

/// Why [`CalibrationMasters::write_bundle`](crate::CalibrationMasters::write_bundle) could not
/// write a bundle, or
/// [`CalibrationMasters::read_bundle`](crate::CalibrationMasters::read_bundle) could not read one.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// The file could not be written.
    #[error("calibration-master bundle: {0}")]
    Write(#[from] WriteError),
    /// A defect's pixel index past the FITS signed 64-bit range.
    #[error("defect index {idx} exceeds the FITS signed 64-bit range")]
    DefectIdxOutOfRange { idx: usize },
    /// A defect map's sensor extent past the FITS signed 64-bit range.
    #[error("defect map of {size} exceeds the FITS signed 64-bit range")]
    DefectExtentOutOfRange { size: Size2us },
    /// The file could not be opened or read.
    #[error("calibration-master bundle: {0}")]
    Io(#[from] io::Error),
    /// The file is not well-formed FITS, or a header value has the wrong type.
    #[error("calibration-master bundle: {0}")]
    Fits(#[from] fits_well::FitsError),
    /// The file does not start with the dataless primary HDU that names a Lumos bundle.
    #[error("not a Lumos calibration-master bundle")]
    NotABundle,
    /// The primary HDU names a bundle but no version of it.
    #[error("the calibration-master bundle states no version")]
    MissingVersion,
    /// The bundle is of another version than this build reads.
    #[error("calibration-master bundle version {found}; this build reads version {expected}")]
    Version { found: i64, expected: i64 },
    /// An HDU's data or header does not match its checksum.
    #[error("HDU {hdu} of the calibration-master bundle fails its checksum")]
    Checksum { hdu: usize },
    /// An extension has no `EXTNAME` to say what it holds.
    #[error("HDU {hdu} of the calibration-master bundle has no EXTNAME")]
    MissingExtname { hdu: usize },
    /// An extension's `EXTNAME` names nothing a bundle holds.
    #[error("unknown calibration-master bundle extension {extname:?}")]
    UnknownExtension { extname: String },
    /// Two extensions name the same component.
    #[error("calibration-master bundle extension {extname:?} appears twice")]
    DuplicateExtension { extname: String },
    /// A bundle holds no flat-dark: the flat is stored prepared, the flat-dark spent on it.
    #[error("the calibration-master bundle holds a flat-dark")]
    FlatDark,
    /// A defect map without the dark or the flat it was detected on.
    #[error("the calibration-master bundle holds a defect map without a dark or a flat")]
    DefectsWithoutMaster,
    /// A flags extension names no image of the file, or is not this format's.
    #[error(transparent)]
    Flags(FitsLoadError),
    /// A master's HDU is not an uncompressed 32-bit float image.
    #[error("the {role} master of the bundle is not an uncompressed BITPIX=-32 image")]
    MasterEncoding { role: MasterRole },
    /// A master's HDU states another role, or another prepared state, than its `EXTNAME`'s.
    #[error("the {role} master of the bundle states another role or prepared state")]
    MasterRecord { role: MasterRole },
    /// A master's HDU could not be decoded.
    #[error("the {role} master of the bundle: {source}")]
    Master {
        role: MasterRole,
        source: FitsLoadError,
    },
    /// The defect map's table is not what a bundle writes.
    #[error("the bundle's defect map: {0}")]
    DefectTable(#[from] DefectTableError),
    /// The masters read do not make a set.
    #[error(transparent)]
    Calibration(#[from] CalibrationError),
    /// The load was cancelled while a master was decoded.
    #[error("calibration-master bundle load cancelled")]
    Cancelled,
}

/// What is wrong with a bundle's defect-map table.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum DefectTableError {
    /// Not a binary table of this format and version.
    #[error("not a Lumos defect table of this version")]
    Format,
    /// The sensor extent is missing, not positive, or overflows.
    #[error("no valid sensor extent")]
    Extent,
    /// A column is missing or of another type.
    #[error("column {col} is missing or of another type")]
    Col { col: &'static str },
    /// A column's length is not the table's row count.
    #[error("the column lengths do not match the row count")]
    ColLen,
    /// A row's kind is neither hot (0) nor cold (1).
    #[error("row {row} has kind {kind}, neither hot (0) nor cold (1)")]
    Kind { row: usize, kind: u8 },
    /// A row's index is negative or lies outside the sensor.
    #[error("row {row} indexes no pixel of the sensor")]
    Idx { row: usize },
    /// A hot row's excess or noise is not a positive finite number, or a cold row's is not NaN.
    #[error("row {row} has an invalid excess or noise")]
    Excess { row: usize },
}
