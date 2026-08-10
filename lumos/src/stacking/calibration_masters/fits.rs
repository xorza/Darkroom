use std::io::{Error as IoError, ErrorKind};
use std::path::Path;

use common::file_utils;
use fits_well::header::Header;
use fits_well::image::Bitpix;
use fits_well::io::{ChecksumStatus, HduKind, SliceReader};
use fits_well::table::{ColumnData, TableBuilder, WriteColumn};
use fits_well::{FitsReader, FitsWriter};

use crate::io::image::cfa::CfaImage;
use crate::io::image::fits::cfa::{
    CFA_FITS_FORMAT, CFA_FITS_VERSION, CfaFitsHdu, CfaFitsHduMetadata,
};
use crate::io::image::fits::decode::read_cfa_hdu;
use crate::io::image::fits::error::fits_to_io;
use crate::math::size2us::Size2us;
use crate::stacking::calibration_masters::CalibrationMasters;
use crate::stacking::calibration_masters::CalibrationSet;
use crate::stacking::calibration_masters::defect_map::DefectMap;
use crate::stacking::calibration_masters::{CalibrationComponent, MasterRole};

const BUNDLE_FORMAT: &str = "CALMASTR";
const DEFECT_FORMAT: &str = "DEFMAP";
const BUNDLE_VERSION: i64 = 1;

/// Where each component's HDU sits in a bundle being read.
#[derive(Debug, Default)]
struct BundleIndices {
    masters: CalibrationSet<Option<usize>>,
    defects: Option<usize>,
}

pub(super) fn save(path: &Path, masters: &CalibrationMasters) -> std::io::Result<()> {
    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        let mut writer = FitsWriter::new(&mut *file).with_checksums();
        writer
            .write_raw_hdu(&bundle_primary_header()?, &[])
            .map_err(fits_to_io)?;

        for (role, master) in masters.masters.iter() {
            let Some(image) = master.as_ref() else {
                continue;
            };
            // The `IMAGETYP` a role's HDU carries is its `EXTNAME` in words, so the two cannot drift.
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
                .write_image_with_header(&encoded.image, &encoded.header)
                .map_err(fits_to_io)?;
        }

        if let Some(defect_map) = &masters.defect_map {
            let encoded = encode_defect_map(defect_map)?;
            writer
                .write_table_with_header(&encoded.table, &encoded.header)
                .map_err(fits_to_io)?;
        }
        Ok(())
    })
}

pub(super) fn load(path: &Path) -> std::io::Result<CalibrationMasters> {
    let bytes = std::fs::read(path)?;
    let mut reader = FitsReader::from_bytes(&bytes).map_err(fits_to_io)?;
    validate_primary(&reader)?;
    verify_checksums(&mut reader)?;
    let indices = bundle_indices(&reader)?;

    let masters = CalibrationMasters {
        masters: indices
            .masters
            .try_map(|role, index| read_master(&mut reader, index, role, path))?,
        defect_map: read_defect_map(&mut reader, indices.defects)?,
    };
    // The same coherence check `from_images` runs, so a bundle read back from disk is exactly as
    // trustworthy as one just built — and neither can exist in a state the other would reject.
    masters
        .validate_dimensions()
        .map_err(|source| invalid_data(source.to_string()))?;
    Ok(masters)
}

fn bundle_primary_header() -> std::io::Result<Header> {
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

fn validate_primary(reader: &SliceReader<'_>) -> std::io::Result<()> {
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

fn verify_checksums(reader: &mut SliceReader<'_>) -> std::io::Result<()> {
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

fn bundle_indices(reader: &SliceReader<'_>) -> std::io::Result<BundleIndices> {
    let mut indices = BundleIndices::default();
    for (index, hdu) in reader.hdus().iter().enumerate().skip(1) {
        let extname = hdu
            .header
            .get_text("EXTNAME")
            .map_err(fits_to_io)?
            .ok_or_else(|| invalid_data(format!("HDU {index} is missing EXTNAME")))?;
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

fn record_index(slot: &mut Option<usize>, index: usize, extname: &str) -> std::io::Result<()> {
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
) -> std::io::Result<Option<CfaImage>> {
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
    if hdu.header.get_text("LUMOSFMT").map_err(fits_to_io)? != Some(CFA_FITS_FORMAT)
        || hdu.header.get_integer("LUMOSVER").map_err(fits_to_io)? != Some(CFA_FITS_VERSION)
        || hdu.header.get_text("LUMROLE").map_err(fits_to_io)? != Some(extname)
    {
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
    read_cfa_hdu(reader, index, path)
        .map(Some)
        .map_err(|source| IoError::new(ErrorKind::InvalidData, source))
}

#[derive(Debug)]
struct EncodedDefectMap {
    table: TableBuilder,
    header: Header,
}

fn encode_defect_map(map: &DefectMap) -> std::io::Result<EncodedDefectMap> {
    let mut kinds = Vec::with_capacity(map.hot_indices.len() + map.cold_indices.len());
    kinds.resize(map.hot_indices.len(), 0);
    kinds.resize(kinds.len() + map.cold_indices.len(), 1);
    let indices = map
        .hot_indices
        .iter()
        .chain(&map.cold_indices)
        .map(|&index| {
            i64::try_from(index)
                .map_err(|_| invalid_data("defect index exceeds the FITS signed-64 range"))
        })
        .collect::<std::io::Result<Vec<_>>>()?;
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
    if let Some(dimensions) = map.dimensions {
        header
            .set(
                "LUMWID",
                i64::try_from(dimensions.width).map_err(|_| {
                    invalid_data("defect-map width exceeds the FITS signed-64 range")
                })?,
            )
            .and_then(|header| {
                header.set(
                    "LUMHEI",
                    i64::try_from(dimensions.height)
                        .map_err(|_| fits_well::FitsError::KeywordOutOfRange { name: "LUMHEI" })?,
                )
            })
            .map_err(fits_to_io)?;
    }
    Ok(EncodedDefectMap { table, header })
}

fn read_defect_map(
    reader: &mut SliceReader<'_>,
    index: Option<usize>,
) -> std::io::Result<Option<DefectMap>> {
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
    let row_count = table.metadata().nrows;
    let kinds = match table
        .column_by_name("KIND")
        .and_then(|column| column.raw())
        .map_err(fits_to_io)?
    {
        ColumnData::Bytes(values) => values,
        _ => return Err(invalid_data("DEFECT_MAP KIND must be a byte column")),
    };
    let indices = match table
        .column_by_name("INDEX")
        .and_then(|column| column.raw())
        .map_err(fits_to_io)?
    {
        ColumnData::I64(values) => values,
        _ => return Err(invalid_data("DEFECT_MAP INDEX must be an int64 column")),
    };
    if kinds.len() != row_count || indices.len() != row_count {
        return Err(invalid_data(
            "DEFECT_MAP column lengths do not match NAXIS2",
        ));
    }
    if dimensions.is_none() && !indices.is_empty() {
        return Err(invalid_data("non-empty DEFECT_MAP is missing dimensions"));
    }

    let pixel_count = dimensions.map(Size2us::pixel_count);
    let mut hot_indices = Vec::new();
    let mut cold_indices = Vec::new();
    for (kind, index) in kinds.into_iter().zip(indices) {
        let index = usize::try_from(index)
            .map_err(|_| invalid_data("DEFECT_MAP contains a negative or oversized index"))?;
        if pixel_count.is_some_and(|count| index >= count) {
            return Err(invalid_data(
                "DEFECT_MAP index lies outside its sensor dimensions",
            ));
        }
        match kind {
            0 => hot_indices.push(index),
            1 => cold_indices.push(index),
            _ => return Err(invalid_data("DEFECT_MAP KIND must be 0 or 1")),
        }
    }
    validate_sorted(&hot_indices, "hot")?;
    validate_sorted(&cold_indices, "cold")?;
    Ok(Some(DefectMap {
        hot_indices,
        cold_indices,
        dimensions,
    }))
}

fn read_defect_dimensions(header: &Header) -> std::io::Result<Option<Size2us>> {
    let width = header.get_integer("LUMWID").map_err(fits_to_io)?;
    let height = header.get_integer("LUMHEI").map_err(fits_to_io)?;
    match (width, height) {
        (None, None) => Ok(None),
        (Some(width), Some(height)) => {
            let width = usize::try_from(width)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| invalid_data("DEFECT_MAP has an invalid width"))?;
            let height = usize::try_from(height)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| invalid_data("DEFECT_MAP has an invalid height"))?;
            width
                .checked_mul(height)
                .ok_or_else(|| invalid_data("DEFECT_MAP dimensions overflow"))?;
            Ok(Some(Size2us::new(width, height)))
        }
        _ => Err(invalid_data(
            "DEFECT_MAP must declare both LUMWID and LUMHEI or neither",
        )),
    }
}

fn validate_sorted(indices: &[usize], kind: &str) -> std::io::Result<()> {
    if indices.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid_data(format!(
            "DEFECT_MAP {kind} indices must be strictly ascending"
        )));
    }
    Ok(())
}

fn invalid_data(message: impl Into<String>) -> IoError {
    IoError::new(ErrorKind::InvalidData, message.into())
}
