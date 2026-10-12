//! The multi-extension FITS bundle a [`CalibrationMasters`] saves to and loads from.
//!
//! A dataless primary HDU tagged with the format and version, one image extension per present
//! master, each followed by a `LUMFLAGS` extension when its flags hold more than `NO_DATA`, and a
//! binary table for the defect map. Every extension is named by
//! [`CalibrationComponent::extname`], which is also what the reader recognizes it by, and every
//! HDU carries a checksum the loader verifies before trusting a byte of it.

use std::fs::File;
use std::io;
use std::io::{Error as IoError, ErrorKind};
use std::path::Path;

use common::file_utils;
use fits_well::header::Header;
use fits_well::image::Bitpix;
use fits_well::io::{ChecksumStatus, HduKind, StreamReader};
use fits_well::table::{ColumnData, TableBuilder, WriteColumn};
use fits_well::{FitsReader, FitsWriter};

use crate::calibration::calibration_component::CalibrationComponent;
use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::defect_map::{DefectMap, HotPixels};
use crate::calibration::error::{BundleError, DefectTableError};
use crate::calibration::master_role::MasterRole;
use crate::calibration::prepared_flat::PreparedFlat;
use crate::image::cfa::cfa_image::CfaImage;
use crate::io::error::ImageError;
use crate::io::fits::cfa::{CfaFitsHdu, CfaFitsHduMetadata};
use crate::io::fits::decode::read_cfa_hdu;
use crate::io::fits::error::fits_to_io;
use crate::io::fits::flags_extension::{FLAGS_EXTNAME, FlagsExtension};
use crate::io::fits::provenance::{FitsChecksumProvenance, FitsChecksumState};
use crate::io::load_context::LoadContext;
use crate::math::size2us::Size2us;

const BUNDLE_FORMAT: &str = "CALMASTR";
const DEFECT_FORMAT: &str = "DEFMAP";
/// 5: each hot pixel of the defect map keeps its excess over the dark around it. Each master's
/// header records what calibration removed from it — the dark whether it lost its bias — and its
/// flags sit in a `LUMFLAGS` extension. A bundle holds no flat-dark.
pub(crate) const BUNDLE_VERSION: i64 = 5;
/// A defect row's `KIND`.
const HOT: u8 = 0;
const COLD: u8 = 1;

/// Where each component's HDU sits in a bundle being read.
#[derive(Debug, Default)]
struct BundleIndices {
    masters: CalibrationSet<Option<usize>>,
    defects: Option<usize>,
}

pub(super) fn save(path: &Path, masters: &CalibrationMasters) -> io::Result<()> {
    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        let mut writer = FitsWriter::new(&mut *file).with_checksums();
        writer
            .write_raw_hdu(&bundle_primary_header()?, &[])
            .map_err(fits_to_io)?;

        let mut flags_written = 0;
        for (role, image) in masters.masters() {
            // The `IMAGETYP` a role's HDU carries is its `EXTNAME` in words, so the two cannot
            // drift.
            let image_type = role.extname().replace('_', " ");
            let encoded = CfaFitsHdu::encode(
                image,
                CfaFitsHduMetadata {
                    extname: Some(role.extname()),
                    image_type: Some(&image_type),
                    prepared: role.prepared(),
                },
            )?;
            writer
                .write_image(&encoded.image, Some(&encoded.header))
                .map_err(fits_to_io)?;
            if let Some(flags) =
                FlagsExtension::encode(image.flags(), Some(role.extname()), flags_written + 1)?
            {
                writer
                    .write_image(&flags.image, Some(&flags.header))
                    .map_err(fits_to_io)?;
                flags_written += 1;
            }
        }

        if let Some(defect_map) = &masters.defect_map {
            let encoded = encode_defect_map(defect_map)?;
            writer
                .write_table(&encoded.table, Some(&encoded.header))
                .map_err(fits_to_io)?;
        }
        Ok(())
    })
}

pub(super) fn load(path: &Path, context: &LoadContext) -> Result<CalibrationMasters, BundleError> {
    // Streamed, not mapped: a bundle is the user's file, and one rewritten in place while it is
    // read must fail the load, not fault the process.
    let mut reader = FitsReader::open(File::open(path)?)?;
    validate_primary(&reader)?;
    verify_checksums(&mut reader)?;
    let indices = bundle_indices(&reader, path)?;

    if indices.masters.flat_dark.is_some() {
        return Err(BundleError::FlatDark);
    }
    if indices.defects.is_some() && indices.masters.dark.is_none() && indices.masters.flat.is_none()
    {
        return Err(BundleError::DefectsWithoutMaster);
    }
    // The same coherence checks `from_images` runs, so a bundle read back from disk is exactly as
    // trustworthy as one just built — and neither can exist in a state the other would reject.
    let masters = CalibrationMasters::assemble(
        read_master(
            &mut reader,
            indices.masters.bias,
            MasterRole::Bias,
            path,
            context,
        )?,
        read_master(
            &mut reader,
            indices.masters.dark,
            MasterRole::Dark,
            path,
            context,
        )?,
        read_master(
            &mut reader,
            indices.masters.flat,
            MasterRole::Flat,
            path,
            context,
        )?
        .map(PreparedFlat::from_divisor),
        read_defect_map(&mut reader, indices.defects)?,
    )?;
    Ok(masters)
}

fn bundle_primary_header() -> io::Result<Header> {
    let mut header = Header::new();
    header
        .set("SIMPLE", true)
        .and_then(|header| header.set("BITPIX", 8))
        .and_then(|header| header.set("NAXIS", 0))
        .and_then(|header| header.set("EXTEND", true))
        .and_then(|header| header.set("LUMOSFMT", BUNDLE_FORMAT))
        .and_then(|header| header.set("LUMOSVER", BUNDLE_VERSION))
        .map_err(fits_to_io)?;
    Ok(header)
}

fn validate_primary(reader: &StreamReader<File>) -> Result<(), BundleError> {
    let primary = reader.hdus().first().ok_or(BundleError::NotABundle)?;
    if primary.kind != HduKind::Primary
        || primary.header.naxis()? != 0
        || primary.header.get_text("LUMOSFMT")? != Some(BUNDLE_FORMAT)
    {
        return Err(BundleError::NotABundle);
    }
    let found = primary
        .header
        .get_integer("LUMOSVER")?
        .ok_or(BundleError::MissingVersion)?;
    if found != BUNDLE_VERSION {
        return Err(BundleError::Version { found });
    }
    Ok(())
}

fn verify_checksums(reader: &mut StreamReader<File>) -> Result<(), BundleError> {
    for hdu in 0..reader.hdus().len() {
        let report = reader.verify_checksum(hdu)?;
        if report.datasum != ChecksumStatus::Valid || report.checksum != ChecksumStatus::Valid {
            return Err(BundleError::Checksum { hdu });
        }
    }
    Ok(())
}

fn bundle_indices(reader: &StreamReader<File>, path: &Path) -> Result<BundleIndices, BundleError> {
    // Each master finds its own flags when it is read; this refuses one that names none.
    FlagsExtension::check_claims(path, reader.hdus()).map_err(BundleError::Flags)?;
    let mut indices = BundleIndices::default();
    for (hdu, unit) in reader.hdus().iter().enumerate().skip(1) {
        let extname = unit
            .header
            .get_text("EXTNAME")?
            .ok_or(BundleError::MissingExtname { hdu })?;
        if extname.eq_ignore_ascii_case(FLAGS_EXTNAME) {
            continue;
        }
        let component = CalibrationComponent::from_extname(&extname.to_ascii_uppercase())
            .ok_or_else(|| BundleError::UnknownExtension {
                extname: extname.to_string(),
            })?;
        let slot = match component {
            CalibrationComponent::Master(role) => indices.masters.get_mut(role),
            CalibrationComponent::Defects => &mut indices.defects,
        };
        if slot.replace(hdu).is_some() {
            return Err(BundleError::DuplicateExtension {
                extname: extname.to_string(),
            });
        }
    }
    Ok(indices)
}

fn read_master(
    reader: &mut StreamReader<File>,
    index: Option<usize>,
    role: MasterRole,
    path: &Path,
    context: &LoadContext,
) -> Result<Option<CfaImage>, BundleError> {
    let Some(index) = index else {
        return Ok(None);
    };
    let hdu = &reader.hdus()[index];
    if hdu.kind != HduKind::Image || hdu.header.bitpix()? != Bitpix::F32 {
        return Err(BundleError::MasterEncoding { role });
    }
    let prepared = hdu.header.get_logical("LUMPREP")?.unwrap_or(false);
    if hdu.header.get_text("LUMROLE")? != Some(role.extname()) || prepared != role.prepared() {
        return Err(BundleError::MasterRecord { role });
    }
    // Every HDU's checksum was verified before any was read.
    let checksum = FitsChecksumProvenance {
        datasum: FitsChecksumState::Valid,
        checksum: FitsChecksumState::Valid,
    };
    read_cfa_hdu(reader, index, path, context, checksum)
        .map(Some)
        .map_err(|source| match source {
            ImageError::Cancelled { .. } => BundleError::Cancelled,
            source => BundleError::Master { role, source },
        })
}

#[derive(Debug)]
struct EncodedDefectMap {
    table: TableBuilder,
    header: Header,
}

fn encode_defect_map(map: &DefectMap) -> io::Result<EncodedDefectMap> {
    let (hot, cold) = (map.hot(), map.cold_indices());
    let rows = hot.indices.len() + cold.len();
    let mut kinds = Vec::with_capacity(rows);
    kinds.resize(hot.indices.len(), HOT);
    kinds.resize(rows, COLD);
    let mut excess = Vec::with_capacity(rows);
    excess.extend_from_slice(&hot.excess);
    excess.resize(rows, f32::NAN);
    let indices = hot
        .indices
        .iter()
        .chain(cold)
        .map(|&index| {
            i64::try_from(index)
                .map_err(|_| invalid_data("defect index exceeds the FITS signed-64 range"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let table = TableBuilder::explicit(
        kinds.len(),
        [
            WriteColumn::scalar("KIND", ColumnData::Bytes(kinds)),
            WriteColumn::scalar("INDEX", ColumnData::I64(indices)),
            WriteColumn::scalar("EXCESS", ColumnData::F32(excess)),
        ],
    )
    .map_err(fits_to_io)?;
    let mut header = Header::new();
    header
        .set("EXTNAME", CalibrationComponent::Defects.extname())
        .and_then(|header| header.set("LUMOSFMT", DEFECT_FORMAT))
        .and_then(|header| header.set("LUMOSVER", BUNDLE_VERSION))
        .map_err(fits_to_io)?;
    let dimensions = map.dimensions();
    let extent = |extent: usize, name: &str| {
        i64::try_from(extent).map_err(|_| {
            invalid_data(format!(
                "defect-map {name} exceeds the FITS signed-64 range"
            ))
        })
    };
    let (width, height) = (
        extent(dimensions.width, "width")?,
        extent(dimensions.height, "height")?,
    );
    header
        .set("LUMWID", width)
        .and_then(|header| header.set("LUMHEI", height))
        .map_err(fits_to_io)?;
    Ok(EncodedDefectMap { table, header })
}

fn read_defect_map(
    reader: &mut StreamReader<File>,
    index: Option<usize>,
) -> Result<Option<DefectMap>, BundleError> {
    let Some(index) = index else {
        return Ok(None);
    };
    let invalid = |error| Err(BundleError::DefectTable(error));
    let header = &reader.hdus()[index].header;
    if reader.hdus()[index].kind != HduKind::BinTable
        || header.get_text("LUMOSFMT")? != Some(DEFECT_FORMAT)
        || header.get_integer("LUMOSVER")? != Some(BUNDLE_VERSION)
    {
        return invalid(DefectTableError::Format);
    }
    let Some(dimensions) = read_defect_dimensions(header)? else {
        return invalid(DefectTableError::Extent);
    };
    let table = reader.read_table(index)?;
    let row_count = table.schema().nrows;
    let column = |name: &'static str| {
        table
            .column_by_name(name)
            .and_then(|column| column.raw())
            .map_err(|_| BundleError::DefectTable(DefectTableError::Column { column: name }))
    };
    let ColumnData::Bytes(kinds) = column("KIND")? else {
        return invalid(DefectTableError::Column { column: "KIND" });
    };
    let ColumnData::I64(indices) = column("INDEX")? else {
        return invalid(DefectTableError::Column { column: "INDEX" });
    };
    let ColumnData::F32(excess) = column("EXCESS")? else {
        return invalid(DefectTableError::Column { column: "EXCESS" });
    };
    if kinds.len() != row_count || indices.len() != row_count || excess.len() != row_count {
        return invalid(DefectTableError::ColumnLength);
    }
    let mut hot = HotPixels::default();
    let mut cold = Vec::new();
    for (row, ((kind, index), excess)) in kinds.into_iter().zip(indices).zip(excess).enumerate() {
        let Some(index) = usize::try_from(index)
            .ok()
            .filter(|&index| index < dimensions.pixel_count())
        else {
            return invalid(DefectTableError::Index { row });
        };
        match kind {
            HOT if excess.is_finite() && excess > 0.0 => {
                hot.indices.push(index);
                hot.excess.push(excess);
            }
            COLD if excess.is_nan() => cold.push(index),
            HOT | COLD => return invalid(DefectTableError::Excess { row }),
            kind => return invalid(DefectTableError::Kind { row, kind }),
        }
    }
    Ok(Some(
        DefectMap::from_lists(dimensions, hot, cold).expect("every index lies on the sensor"),
    ))
}

/// The sensor extent the table states; `None` when an extent is missing, not positive, or the two
/// overflow.
fn read_defect_dimensions(header: &Header) -> Result<Option<Size2us>, BundleError> {
    let extent = |name: &str| -> Result<Option<usize>, BundleError> {
        Ok(header
            .get_integer(name)?
            .and_then(|value| usize::try_from(value).ok())
            .filter(|&value| value > 0))
    };
    let (Some(width), Some(height)) = (extent("LUMWID")?, extent("LUMHEI")?) else {
        return Ok(None);
    };
    Ok(width
        .checked_mul(height)
        .map(|_| Size2us::new(width, height)))
}

fn invalid_data(message: impl Into<String>) -> IoError {
    IoError::new(ErrorKind::InvalidData, message.into())
}
