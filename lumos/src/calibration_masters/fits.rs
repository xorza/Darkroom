//! The multi-extension FITS bundle a [`CalibrationMasters`] saves to and loads from.
//!
//! A dataless primary HDU tagged with the format and version, one image extension per present
//! master, each followed by a `LUMFLAGS` extension when its flags hold more than `NO_DATA`, and a
//! binary table for the defect map. Every extension is named by
//! [`CalibrationComponent::extname`], which is also what the reader recognizes it by, and every
//! HDU carries a checksum the loader verifies before trusting a byte of it.

use std::fs;
use std::io;
use std::io::{Error as IoError, ErrorKind};
use std::path::Path;

use common::file_utils;
use fits_well::header::Header;
use fits_well::image::Bitpix;
use fits_well::io::{ChecksumStatus, HduKind, SliceReader};
use fits_well::table::{ColumnData, TableBuilder, WriteColumn};
use fits_well::{FitsReader, FitsWriter};

use crate::calibration_masters::CalibrationMasters;
use crate::calibration_masters::calibration_component::CalibrationComponent;
use crate::calibration_masters::calibration_set::CalibrationSet;
use crate::calibration_masters::defect_map::DefectMap;
use crate::calibration_masters::master_role::MasterRole;
use crate::calibration_masters::prepared_flat::PreparedFlat;
use crate::io::image::cfa::CfaImage;
use crate::io::image::error::ImageError;
use crate::io::image::fits::cfa::{CfaFitsHdu, CfaFitsHduMetadata};
use crate::io::image::fits::decode::read_cfa_hdu;
use crate::io::image::fits::error::fits_to_io;
use crate::io::image::fits::flags_extension::{FLAGS_EXTNAME, FlagsExtension};
use crate::io::image::fits::provenance::{FitsChecksumProvenance, FitsChecksumState};
use crate::io::image::load_context::LoadContext;
use crate::math::size2us::Size2us;

const BUNDLE_FORMAT: &str = "CALMASTR";
const DEFECT_FORMAT: &str = "DEFMAP";
/// 4: each master's header records what calibration removed from it — the dark whether it lost its
/// bias — and its flags sit in a `LUMFLAGS` extension. A bundle holds no flat-dark.
const BUNDLE_VERSION: i64 = 4;

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

pub(super) fn load(path: &Path, context: &LoadContext) -> io::Result<CalibrationMasters> {
    let bytes = fs::read(path)?;
    let mut reader = FitsReader::from_bytes(&bytes).map_err(fits_to_io)?;
    validate_primary(&reader)?;
    verify_checksums(&mut reader)?;
    let indices = bundle_indices(&reader, path)?;

    if indices.masters.flat_dark.is_some() {
        return Err(invalid_data(
            "a calibration-master bundle holds no flat-dark: the flat is stored prepared",
        ));
    }
    let masters = CalibrationMasters {
        bias: read_master(
            &mut reader,
            indices.masters.bias,
            MasterRole::Bias,
            path,
            context,
        )?,
        dark: read_master(
            &mut reader,
            indices.masters.dark,
            MasterRole::Dark,
            path,
            context,
        )?,
        flat: read_master(
            &mut reader,
            indices.masters.flat,
            MasterRole::Flat,
            path,
            context,
        )?
        .map(PreparedFlat::from_divisor),
        defect_map: read_defect_map(&mut reader, indices.defects)?,
    };
    // The same coherence checks `from_images` runs, so a bundle read back from disk is exactly as
    // trustworthy as one just built — and neither can exist in a state the other would reject.
    masters
        .validate_dimensions()
        .and_then(|()| masters.validate_records())
        .map_err(|source| IoError::new(ErrorKind::InvalidData, source))?;
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

fn validate_primary(reader: &SliceReader<'_>) -> io::Result<()> {
    let Some(primary) = reader.hdus().first() else {
        return Err(invalid_data("calibration-master FITS has no primary HDU"));
    };
    if primary.kind != HduKind::Primary || primary.header.naxis().map_err(fits_to_io)? != 0 {
        return Err(invalid_data(
            "calibration-master FITS must start with a dataless primary HDU",
        ));
    }
    if primary.header.get_text("LUMOSFMT").map_err(fits_to_io)? != Some(BUNDLE_FORMAT) {
        return Err(invalid_data("not a Lumos calibration-master FITS bundle"));
    }
    let version = primary
        .header
        .get_integer("LUMOSVER")
        .map_err(fits_to_io)?
        .ok_or_else(|| invalid_data("calibration-master FITS is missing LUMOSVER"))?;
    if version != BUNDLE_VERSION {
        return Err(invalid_data(format!(
            "unsupported calibration-master FITS version {version}; expected {BUNDLE_VERSION}"
        )));
    }
    Ok(())
}

fn verify_checksums(reader: &mut SliceReader<'_>) -> io::Result<()> {
    for index in 0..reader.hdus().len() {
        let report = reader.verify_checksum(index).map_err(fits_to_io)?;
        if report.datasum != ChecksumStatus::Valid || report.checksum != ChecksumStatus::Valid {
            return Err(invalid_data(format!(
                "calibration-master FITS checksum mismatch in HDU {index}"
            )));
        }
    }
    Ok(())
}

fn bundle_indices(reader: &SliceReader<'_>, path: &Path) -> io::Result<BundleIndices> {
    // Each master finds its own flags when it is read; this refuses one that names none.
    FlagsExtension::check_claims(path, reader.hdus())
        .map_err(|source| IoError::new(ErrorKind::InvalidData, source))?;
    let mut indices = BundleIndices::default();
    for (index, hdu) in reader.hdus().iter().enumerate().skip(1) {
        let extname = hdu
            .header
            .get_text("EXTNAME")
            .map_err(fits_to_io)?
            .ok_or_else(|| invalid_data(format!("HDU {index} is missing EXTNAME")))?;
        if extname.eq_ignore_ascii_case(FLAGS_EXTNAME) {
            continue;
        }
        let component = CalibrationComponent::from_extname(&extname.to_ascii_uppercase())
            .ok_or_else(|| {
                invalid_data(format!(
                    "unknown calibration-master FITS extension {extname:?}"
                ))
            })?;
        let slot = match component {
            CalibrationComponent::Master(role) => indices.masters.get_mut(role),
            CalibrationComponent::Defects => &mut indices.defects,
        };
        record_index(slot, index, extname)?;
    }
    Ok(indices)
}

fn record_index(slot: &mut Option<usize>, index: usize, extname: &str) -> io::Result<()> {
    if slot.replace(index).is_some() {
        return Err(invalid_data(format!(
            "duplicate calibration-master FITS extension {extname:?}"
        )));
    }
    Ok(())
}

fn read_master(
    reader: &mut SliceReader<'_>,
    index: Option<usize>,
    role: MasterRole,
    path: &Path,
    context: &LoadContext,
) -> io::Result<Option<CfaImage>> {
    let Some(index) = index else {
        return Ok(None);
    };
    let extname = role.extname();
    let hdu = &reader.hdus()[index];
    if hdu.kind != HduKind::Image || hdu.header.bitpix().map_err(fits_to_io)? != Bitpix::F32 {
        return Err(invalid_data(format!(
            "{extname} must be an uncompressed BITPIX=-32 image extension"
        )));
    }
    if hdu.header.get_text("LUMROLE").map_err(fits_to_io)? != Some(extname) {
        return Err(invalid_data(format!(
            "{extname} has invalid Lumos CFA metadata"
        )));
    }
    let prepared = hdu
        .header
        .get_logical("LUMPREP")
        .map_err(fits_to_io)?
        .unwrap_or(false);
    if prepared != role.prepared() {
        return Err(invalid_data(format!(
            "{extname} has an invalid prepared-master state"
        )));
    }
    // Every HDU's checksum was verified before any was read.
    let checksum = FitsChecksumProvenance {
        datasum: FitsChecksumState::Valid,
        checksum: FitsChecksumState::Valid,
    };
    read_cfa_hdu(reader, index, path, context, checksum)
        .map(Some)
        .map_err(|source| {
            let kind = match source {
                ImageError::Cancelled { .. } => ErrorKind::Interrupted,
                _ => ErrorKind::InvalidData,
            };
            IoError::new(kind, source)
        })
}

#[derive(Debug)]
struct EncodedDefectMap {
    table: TableBuilder,
    header: Header,
}

fn encode_defect_map(map: &DefectMap) -> io::Result<EncodedDefectMap> {
    let (hot, cold) = (map.hot_indices(), map.cold_indices());
    let mut kinds = Vec::with_capacity(hot.len() + cold.len());
    kinds.resize(hot.len(), 0);
    kinds.resize(kinds.len() + cold.len(), 1);
    let indices = hot
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
    reader: &mut SliceReader<'_>,
    index: Option<usize>,
) -> io::Result<Option<DefectMap>> {
    let Some(index) = index else {
        return Ok(None);
    };
    let header = &reader.hdus()[index].header;
    if reader.hdus()[index].kind != HduKind::BinTable
        || header.get_text("LUMOSFMT").map_err(fits_to_io)? != Some(DEFECT_FORMAT)
        || header.get_integer("LUMOSVER").map_err(fits_to_io)? != Some(BUNDLE_VERSION)
    {
        return Err(invalid_data("DEFECT_MAP has invalid Lumos table metadata"));
    }
    let dimensions = read_defect_dimensions(header)?;
    let table = reader.read_table(index).map_err(fits_to_io)?;
    let row_count = table.schema().nrows;
    let ColumnData::Bytes(kinds) = table
        .column_by_name("KIND")
        .and_then(|column| column.raw())
        .map_err(fits_to_io)?
    else {
        return Err(invalid_data("DEFECT_MAP KIND must be a byte column"));
    };
    let ColumnData::I64(indices) = table
        .column_by_name("INDEX")
        .and_then(|column| column.raw())
        .map_err(fits_to_io)?
    else {
        return Err(invalid_data("DEFECT_MAP INDEX must be an int64 column"));
    };
    if kinds.len() != row_count || indices.len() != row_count {
        return Err(invalid_data(
            "DEFECT_MAP column lengths do not match NAXIS2",
        ));
    }
    let mut hot_indices = Vec::new();
    let mut cold_indices = Vec::new();
    for (kind, index) in kinds.into_iter().zip(indices) {
        let index = usize::try_from(index)
            .map_err(|_| invalid_data("DEFECT_MAP contains a negative or oversized index"))?;
        match kind {
            0 => hot_indices.push(index),
            1 => cold_indices.push(index),
            _ => return Err(invalid_data("DEFECT_MAP KIND must be 0 or 1")),
        }
    }
    DefectMap::from_indices(dimensions, hot_indices, cold_indices)
        .map(Some)
        .ok_or_else(|| invalid_data("DEFECT_MAP index lies outside its sensor dimensions"))
}

fn read_defect_dimensions(header: &Header) -> io::Result<Size2us> {
    let extent = |name: &str| -> io::Result<usize> {
        header
            .get_integer(name)
            .map_err(fits_to_io)?
            .and_then(|value| usize::try_from(value).ok())
            .filter(|&value| value > 0)
            .ok_or_else(|| invalid_data(format!("DEFECT_MAP has no valid {name}")))
    };
    let (width, height) = (extent("LUMWID")?, extent("LUMHEI")?);
    width
        .checked_mul(height)
        .ok_or_else(|| invalid_data("DEFECT_MAP dimensions overflow"))?;
    Ok(Size2us::new(width, height))
}

fn invalid_data(message: impl Into<String>) -> IoError {
    IoError::new(ErrorKind::InvalidData, message.into())
}
