//! The multi-extension FITS bundle a [`CalibrationMasters`] is written to and read from.
//!
//! A dataless primary HDU tagged with the format and version, one image extension per present
//! master, each followed by a `LUMFLAGS` extension when its flags hold more than `NO_DATA`, and a
//! binary table for the defect map. Every extension is named by
//! [`CalibrationComponent::extname`], which is also what the reader recognizes it by, and every
//! HDU carries a checksum the loader verifies before trusting a byte of it.

pub(crate) mod error;

use std::fs::File;
use std::path::Path;

use common::file_utils;
use fits_well::header::Header;
use fits_well::image::Bitpix;
use fits_well::io::{ChecksumStatus, HduKind, StreamReader};
use fits_well::table::{Builder as TableBuilder, ColumnData, WriteColumn};
use fits_well::{FitsReader, FitsWriter};

use crate::calibration::calibration_component::CalibrationComponent;
use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::defect_map::{DefectMap, HotPixels};
use crate::calibration::master_role::MasterRole;
use crate::calibration::prepared_flat::PreparedFlat;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::fits_checksum_provenance::{FitsChecksumProvenance, FitsChecksumState};
use crate::io::error::{ImageError, WriteError};
use crate::io::fits::calibration_bundle::error::{BundleError, DefectTableError};
use crate::io::fits::cfa::{CfaFitsHdu, CfaFitsHduMetadata};
use crate::io::fits::decode::selected_fits::{Checksum, SelectedFits};
use crate::io::fits::error::FitsLoadError;
use crate::io::fits::flags_extension::{FLAGS_EXTNAME, FlagsExtension};
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD};
use crate::io::load_context::LoadContext;
use crate::math::size2us::Size2us;

const BUNDLE_FORMAT: &str = "CALMASTR";
const DEFECT_FORMAT: &str = "DEFMAP";
/// 6: each hot pixel of the defect map keeps its excess over the dark around it. Each master's
/// header records what calibration removed from it — the dark whether it lost its bias — and what
/// a master of dark signal taken from it was compared on, and its flags sit in a `LUMFLAGS`
/// extension. A bundle holds no flat-dark.
const BUNDLE_VERSION: i64 = 6;
/// A defect row's `KIND`.
const HOT: u8 = 0;
const COLD: u8 = 1;

/// Where each component's HDU sits in a bundle being read.
#[derive(Debug, Default)]
struct BundleIndices {
    masters: CalibrationSet<Option<usize>>,
    defects: Option<usize>,
}

/// The defect map is encoded first, so a map the format cannot state writes no file.
pub(crate) fn save(path: &Path, masters: &CalibrationMasters) -> Result<(), BundleError> {
    let defects = masters.defect_map().map(encode_defect_map).transpose()?;
    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        let mut writer = FitsWriter::new(&mut *file).with_checksums();
        writer.write_raw_hdu(&bundle_primary_header()?, &[])?;

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
            writer.write_image(&encoded.image, Some(&encoded.header))?;
            if let Some(flags) =
                FlagsExtension::encode(image.flags(), Some(role.extname()), flags_written + 1)?
            {
                writer.write_image(&flags.image, Some(&flags.header))?;
                flags_written += 1;
            }
        }

        if let Some(encoded) = &defects {
            writer.write_table(&encoded.table, Some(&encoded.header))?;
        }
        Ok::<_, WriteError>(())
    })?;
    Ok(())
}

pub(crate) fn load(path: &Path, context: &LoadContext) -> Result<CalibrationMasters, BundleError> {
    // Streamed, not mapped: a bundle is the user's file, and one rewritten in place while it is
    // read must fail the load, not fault the process.
    let mut bundle = BundleReader {
        reader: FitsReader::open(File::open(path)?)?,
        path,
        context,
    };
    bundle.validate_primary()?;
    bundle.verify_checksums()?;
    let indices = bundle.indices()?;

    if indices.masters.flat_dark.is_some() {
        return Err(BundleError::FlatDark);
    }
    if indices.defects.is_some() && indices.masters.dark.is_none() && indices.masters.flat.is_none()
    {
        return Err(BundleError::DefectsWithoutMaster);
    }
    // The same coherence checks `from_images` runs, so a bundle read back from disk is exactly as
    // trustworthy as one just built — and neither can exist in a state the other would reject.
    let masters = CalibrationMasters::builder()
        .bias(bundle.master(indices.masters.bias, MasterRole::Bias)?)
        .dark(bundle.master(indices.masters.dark, MasterRole::Dark)?)
        .flat(
            bundle
                .master(indices.masters.flat, MasterRole::Flat)?
                .map(PreparedFlat::from_divisor),
        )
        .defect_map(bundle.defect_map(indices.defects)?)
        .build()?;
    Ok(masters)
}

fn bundle_primary_header() -> Result<Header, WriteError> {
    let mut header = Header::new();
    header
        .set("SIMPLE", true)
        .and_then(|header| header.set("BITPIX", 8))
        .and_then(|header| header.set("NAXIS", 0))
        .and_then(|header| header.set("EXTEND", true))
        .and_then(|header| header.set(FORMAT_KEYWORD, BUNDLE_FORMAT))
        .and_then(|header| header.set(VERSION_KEYWORD, BUNDLE_VERSION))?;
    Ok(header)
}

/// A bundle being read: its file's HDUs, the path they were read from, and the load's context.
#[derive(Debug)]
struct BundleReader<'a> {
    reader: StreamReader<File>,
    path: &'a Path,
    context: &'a LoadContext,
}

impl BundleReader<'_> {
    fn validate_primary(&self) -> Result<(), BundleError> {
        let primary = self.reader.hdus().first().ok_or(BundleError::NotABundle)?;
        if primary.kind != HduKind::Primary
            || primary.header.naxis()? != 0
            || primary.header.get_text(FORMAT_KEYWORD)? != Some(BUNDLE_FORMAT)
        {
            return Err(BundleError::NotABundle);
        }
        let found = primary
            .header
            .get_integer(VERSION_KEYWORD)?
            .ok_or(BundleError::MissingVersion)?;
        if found != BUNDLE_VERSION {
            return Err(BundleError::Version {
                found,
                expected: BUNDLE_VERSION,
            });
        }
        Ok(())
    }

    fn verify_checksums(&mut self) -> Result<(), BundleError> {
        for hdu in 0..self.reader.hdus().len() {
            let report = self.reader.verify_checksum(hdu)?;
            if report.datasum != ChecksumStatus::Valid || report.checksum != ChecksumStatus::Valid {
                return Err(BundleError::Checksum { hdu });
            }
        }
        Ok(())
    }

    /// Where each component's HDU sits.
    fn indices(&self) -> Result<BundleIndices, BundleError> {
        // Each master finds its own flags when it is read; this refuses one that names none.
        FlagsExtension::check_claims(self.reader.hdus())
            .map_err(|error| BundleError::Flags(ImageError::fits_load(self.path, error)))?;
        let mut indices = BundleIndices::default();
        for (hdu, unit) in self.reader.hdus().iter().enumerate().skip(1) {
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

    /// The `role` master at HDU `index`, when the bundle holds one.
    fn master(
        &mut self,
        index: Option<usize>,
        role: MasterRole,
    ) -> Result<Option<CfaImage>, BundleError> {
        let Some(index) = index else {
            return Ok(None);
        };
        let hdu = &self.reader.hdus()[index];
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
        let context = self.context;
        let mut master = || {
            let selected = SelectedFits::lumos_cfa(&mut self.reader, index, context)?;
            let cfa_type = selected.require_cfa()?;
            selected
                .read(context, Checksum::Verified(checksum))?
                .into_cfa(cfa_type)
        };
        master().map(Some).map_err(|source| match source {
            FitsLoadError::Cancelled => BundleError::Cancelled,
            source => BundleError::Master {
                role,
                source: ImageError::fits_load(self.path, source),
            },
        })
    }

    /// The defect map at HDU `index`, when the bundle holds one.
    fn defect_map(&mut self, index: Option<usize>) -> Result<Option<DefectMap>, BundleError> {
        let Some(index) = index else {
            return Ok(None);
        };
        let header = &self.reader.hdus()[index].header;
        if self.reader.hdus()[index].kind != HduKind::BinTable
            || header.get_text(FORMAT_KEYWORD)? != Some(DEFECT_FORMAT)
            || header.get_integer(VERSION_KEYWORD)? != Some(BUNDLE_VERSION)
        {
            return Err(DefectTableError::Format.into());
        }
        let Some(dimensions) = read_defect_dimensions(header)? else {
            return Err(DefectTableError::Extent.into());
        };
        let table = self.reader.read_table(index)?;
        let row_count = table.schema().nrows;
        let column = |name: &'static str| {
            table
                .column_by_name(name)
                .and_then(|column| column.raw())
                .map_err(|_| DefectTableError::Column { column: name })
        };
        let ColumnData::Bytes(kinds) = column("KIND")? else {
            return Err(DefectTableError::Column { column: "KIND" }.into());
        };
        let ColumnData::I64(indices) = column("INDEX")? else {
            return Err(DefectTableError::Column { column: "INDEX" }.into());
        };
        let ColumnData::F32(excess) = column("EXCESS")? else {
            return Err(DefectTableError::Column { column: "EXCESS" }.into());
        };
        if kinds.len() != row_count || indices.len() != row_count || excess.len() != row_count {
            return Err(DefectTableError::ColumnLength.into());
        }
        let mut hot = HotPixels::default();
        let mut cold = Vec::new();
        for (row, ((kind, index), excess)) in kinds.into_iter().zip(indices).zip(excess).enumerate()
        {
            let Some(index) = usize::try_from(index)
                .ok()
                .filter(|&index| index < dimensions.pixel_count())
            else {
                return Err(DefectTableError::Index { row }.into());
            };
            match kind {
                HOT if excess.is_finite() && excess > 0.0 => {
                    hot.indices.push(index);
                    hot.excess.push(excess);
                }
                COLD if excess.is_nan() => cold.push(index),
                HOT | COLD => return Err(DefectTableError::Excess { row }.into()),
                kind => return Err(DefectTableError::Kind { row, kind }.into()),
            }
        }
        Ok(Some(
            DefectMap::from_lists(dimensions, hot, cold).expect("every index lies on the sensor"),
        ))
    }
}

#[derive(Debug)]
struct EncodedDefectMap {
    table: TableBuilder,
    header: Header,
}

fn encode_defect_map(map: &DefectMap) -> Result<EncodedDefectMap, BundleError> {
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
            i64::try_from(index).map_err(|_| BundleError::DefectIndexOutOfRange { index })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let table = TableBuilder::explicit(
        kinds.len(),
        [
            WriteColumn::scalar("KIND", ColumnData::Bytes(kinds)),
            WriteColumn::scalar("INDEX", ColumnData::I64(indices)),
            WriteColumn::scalar("EXCESS", ColumnData::F32(excess)),
        ],
    )?;
    let mut header = Header::new();
    header
        .set("EXTNAME", CalibrationComponent::Defects.extname())
        .and_then(|header| header.set(FORMAT_KEYWORD, DEFECT_FORMAT))
        .and_then(|header| header.set(VERSION_KEYWORD, BUNDLE_VERSION))?;
    let dimensions = map.dimensions();
    let (Ok(width), Ok(height)) = (
        i64::try_from(dimensions.width),
        i64::try_from(dimensions.height),
    ) else {
        return Err(BundleError::DefectExtentOutOfRange {
            width: dimensions.width,
            height: dimensions.height,
        });
    };
    header
        .set("LUMWID", width)
        .and_then(|header| header.set("LUMHEI", height))?;
    Ok(EncodedDefectMap { table, header })
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

#[cfg(test)]
mod tests {
    use crate::calibration::defect_map::DefectMap;
    use crate::io::fits::calibration_bundle::encode_defect_map;
    use crate::io::fits::calibration_bundle::error::BundleError;
    use crate::math::size2us::Size2us;

    /// A sensor extent past `i64::MAX` is refused with both edges, not written as a wrapped
    /// `LUMWID`; a zero height keeps the map's mask empty.
    #[test]
    fn a_defect_extent_past_the_fits_range_is_refused() {
        let width = usize::try_from(i64::MAX).unwrap() + 1;
        let map = DefectMap::new(Size2us::new(width, 0));
        assert!(matches!(
            encode_defect_map(&map),
            Err(BundleError::DefectExtentOutOfRange { width: w, height: 0 }) if w == width
        ));
    }
}
