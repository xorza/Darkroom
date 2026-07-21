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
    CFA_FITS_FORMAT, CFA_FITS_VERSION, CfaFitsHduMetadata, encode_cfa_hdu,
};
use crate::io::image::fits::decode::read_cfa_hdu;
use crate::io::image::fits::error::fits_to_io;
use crate::math::vec2us::Vec2us;
use crate::stacking::calibration_masters::CalibrationMasters;
use crate::stacking::calibration_masters::defect_map::DefectMap;

const BUNDLE_FORMAT: &str = "CALMASTR";
const DEFECT_FORMAT: &str = "DEFMAP";
const BUNDLE_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy)]
enum MasterRole {
    Dark,
    Flat,
    Bias,
    FlatDark,
}

impl MasterRole {
    fn extname(self) -> &'static str {
        match self {
            Self::Dark => "MASTER_DARK",
            Self::Flat => "MASTER_FLAT",
            Self::Bias => "MASTER_BIAS",
            Self::FlatDark => "MASTER_FLAT_DARK",
        }
    }

    fn image_type(self) -> &'static str {
        match self {
            Self::Dark => "MASTER DARK",
            Self::Flat => "MASTER FLAT",
            Self::Bias => "MASTER BIAS",
            Self::FlatDark => "MASTER FLAT DARK",
        }
    }

    fn prepared(self) -> bool {
        matches!(self, Self::Flat)
    }
}

#[derive(Debug)]
struct MasterToWrite<'a> {
    role: MasterRole,
    image: Option<&'a CfaImage>,
}

#[derive(Debug, Default)]
struct BundleIndices {
    dark: Option<usize>,
    flat: Option<usize>,
    bias: Option<usize>,
    flat_dark: Option<usize>,
    defects: Option<usize>,
}

pub(crate) fn save(path: &Path, masters: &CalibrationMasters) -> std::io::Result<()> {
    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        let mut writer = FitsWriter::new(&mut *file).with_checksums();
        writer
            .write_raw_hdu(&bundle_primary_header()?, &[])
            .map_err(fits_to_io)?;

        for master in [
            MasterToWrite {
                role: MasterRole::Dark,
                image: masters.dark.as_ref(),
            },
            MasterToWrite {
                role: MasterRole::Flat,
                image: masters.flat.as_ref(),
            },
            MasterToWrite {
                role: MasterRole::Bias,
                image: masters.bias.as_ref(),
            },
            MasterToWrite {
                role: MasterRole::FlatDark,
                image: masters.flat_dark.as_ref(),
            },
        ] {
            let Some(image) = master.image else {
                continue;
            };
            let encoded = encode_cfa_hdu(
                image,
                CfaFitsHduMetadata {
                    extname: Some(master.role.extname()),
                    image_type: Some(master.role.image_type()),
                    prepared: master.role.prepared(),
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

pub(crate) fn load(path: &Path) -> std::io::Result<CalibrationMasters> {
    let bytes = std::fs::read(path)?;
    let mut reader = FitsReader::from_bytes(&bytes).map_err(fits_to_io)?;
    validate_primary(&reader)?;
    verify_checksums(&mut reader)?;
    let indices = bundle_indices(&reader)?;

    let masters = CalibrationMasters {
        dark: read_master(&mut reader, indices.dark, MasterRole::Dark, path)?,
        flat: read_master(&mut reader, indices.flat, MasterRole::Flat, path)?,
        bias: read_master(&mut reader, indices.bias, MasterRole::Bias, path)?,
        flat_dark: read_master(&mut reader, indices.flat_dark, MasterRole::FlatDark, path)?,
        defect_map: read_defect_map(&mut reader, indices.defects)?,
    };
    validate_dimensions(&masters)?;
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
        match extname.to_ascii_uppercase().as_str() {
            "MASTER_DARK" => record_index(&mut indices.dark, index, extname)?,
            "MASTER_FLAT" => record_index(&mut indices.flat, index, extname)?,
            "MASTER_BIAS" => record_index(&mut indices.bias, index, extname)?,
            "MASTER_FLAT_DARK" => record_index(&mut indices.flat_dark, index, extname)?,
            "DEFECT_MAP" => record_index(&mut indices.defects, index, extname)?,
            _ => {
                return Err(invalid_data(format!(
                    "unknown calibration-master FITS extension {extname:?}"
                )));
            }
        }
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
    let hdu = &reader.hdus()[index];
    if hdu.kind != HduKind::Image || hdu.header.bitpix().map_err(fits_to_io)? != Bitpix::F32 {
        return Err(invalid_data(format!(
            "{} must be an uncompressed BITPIX=-32 image extension",
            role.extname()
        )));
    }
    if hdu.header.get_text("LUMOSFMT").map_err(fits_to_io)? != Some(CFA_FITS_FORMAT)
        || hdu.header.get_integer("LUMOSVER").map_err(fits_to_io)? != Some(CFA_FITS_VERSION)
        || hdu.header.get_text("LUMROLE").map_err(fits_to_io)? != Some(role.extname())
    {
        return Err(invalid_data(format!(
            "{} has invalid Lumos CFA metadata",
            role.extname()
        )));
    }
    let prepared = hdu
        .header
        .get_logical("LUMPREP")
        .map_err(fits_to_io)?
        .unwrap_or(false);
    if prepared != role.prepared() {
        return Err(invalid_data(format!(
            "{} has an invalid prepared-master state",
            role.extname()
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
        .set("EXTNAME", "DEFECT_MAP")
        .and_then(|header| header.set("LUMOSFMT", DEFECT_FORMAT))
        .and_then(|header| header.set("LUMOSVER", BUNDLE_VERSION))
        .map_err(fits_to_io)?;
    if let Some(dimensions) = map.dimensions {
        header
            .set(
                "LUMWID",
                i64::try_from(dimensions.x).map_err(|_| {
                    invalid_data("defect-map width exceeds the FITS signed-64 range")
                })?,
            )
            .and_then(|header| {
                header.set(
                    "LUMHEI",
                    i64::try_from(dimensions.y)
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

    let pixel_count = dimensions.map(|dimensions| dimensions.x * dimensions.y);
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

fn read_defect_dimensions(header: &Header) -> std::io::Result<Option<Vec2us>> {
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
            Ok(Some(Vec2us::new(width, height)))
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

fn validate_dimensions(masters: &CalibrationMasters) -> std::io::Result<()> {
    let mut dimensions = None;
    for master in [
        masters.dark.as_ref(),
        masters.flat.as_ref(),
        masters.bias.as_ref(),
        masters.flat_dark.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        let current = Vec2us::new(master.data.width(), master.data.height());
        match dimensions {
            None => dimensions = Some(current),
            Some(expected) if expected == current => {}
            Some(_) => {
                return Err(invalid_data(
                    "calibration-master FITS images have inconsistent dimensions",
                ));
            }
        }
    }
    if let Some(defect_dimensions) = masters.defect_map.as_ref().and_then(|map| map.dimensions)
        && dimensions.is_some_and(|image_dimensions| image_dimensions != defect_dimensions)
    {
        return Err(invalid_data(
            "DEFECT_MAP dimensions do not match the calibration masters",
        ));
    }
    Ok(())
}

fn invalid_data(message: impl Into<String>) -> IoError {
    IoError::new(ErrorKind::InvalidData, message.into())
}
