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

use common::fs;
use fits_well::header::Header;
use fits_well::image::Bitpix;
use fits_well::io::{ChecksumStatus, HduKind, StreamReader};
use fits_well::table::{Builder as TableBuilder, ColumnData, Table, WriteColumn};
use fits_well::{FitsReader, FitsWriter};

use crate::calibration::calibration_component::CalibrationComponent;
use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::defect_map::{DefectMap, HotPixels};
use crate::calibration::master_role::MasterRole;
use crate::calibration::prepared_flat::PreparedFlat;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::fits_checksum_provenance::{FitsChecksumProvenance, FitsChecksumState};
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::io::error::{ImageError, WriteError};
use crate::io::fits::calibration_bundle::error::{BundleError, DefectTableError};
use crate::io::fits::cfa::{self, CfaFitsHduMetadata};
use crate::io::fits::decode::selected_fits::{Checksum, SelectedFits};
use crate::io::fits::error::FitsLoadError;
use crate::io::fits::flags_extension::{self, FLAGS_EXTNAME};
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD};
use crate::io::load_ctx::LoadCtx;
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
struct BundleIdxs {
    masters: CalibrationSet<Option<usize>>,
    defects: Option<usize>,
}

/// The defect map is encoded first, so a map the format cannot state writes no file.
pub(crate) fn save(path: &Path, masters: &CalibrationMasters) -> Result<(), BundleError> {
    let defects = masters.defect_map().map(encode_defect_map).transpose()?;
    fs::publish(path, fs::PublicationMode::Durable, |file| {
        let mut writer = FitsWriter::new(&mut *file).with_checksums();
        writer.write_empty_primary(Some(&bundle_primary_header()?))?;

        let mut flags_written = 0;
        for (role, image) in masters.masters() {
            // The `IMAGETYP` a role's HDU carries is its `EXTNAME` in words, so the two cannot
            // drift.
            let image_type = role.extname().replace('_', " ");
            cfa::hdu(
                image,
                CfaFitsHduMetadata {
                    extname: Some(role.extname()),
                    image_type: Some(&image_type),
                    prepared: role.prepared(),
                },
            )?
            .write(&mut writer)?;
            if let Some(flags) =
                flags_extension::encode(image.flags(), Some(role.extname()), flags_written + 1)?
            {
                flags.write(&mut writer)?;
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

pub(crate) fn load(path: &Path, ctx: &LoadCtx) -> Result<CalibrationMasters, BundleError> {
    // Streamed, not mapped: a bundle is the user's file, and one rewritten in place while it is
    // read must fail the load, not fault the process.
    let mut bundle = BundleReader {
        reader: FitsReader::open(fs::open(path)?)?,
        path,
        ctx,
    };
    bundle.validate_primary()?;
    bundle.verify_checksums()?;
    let idxs = bundle.idxs()?;

    if idxs.masters.flat_dark.is_some() {
        return Err(BundleError::FlatDark);
    }
    if idxs.defects.is_some() && idxs.masters.dark.is_none() && idxs.masters.flat.is_none() {
        return Err(BundleError::DefectsWithoutMaster);
    }
    let mut master = |role| {
        (*idxs.masters.get(role))
            .map(|idx| bundle.master(idx, role))
            .transpose()
    };
    let bias = master(MasterRole::Bias)?;
    let dark = master(MasterRole::Dark)?;
    let flat = master(MasterRole::Flat)?;
    let defects = idxs.defects.map(|idx| bundle.defect_map(idx)).transpose()?;
    // The same coherence checks `from_images` runs, so a bundle read back from disk is exactly as
    // trustworthy as one just built — and neither can exist in a state the other would reject.
    let masters = CalibrationMasters::builder()
        .bias(bias)
        .dark(dark)
        .flat(flat.map(PreparedFlat::from_divisor))
        .defect_map(defects)
        .build()?;
    Ok(masters)
}

fn bundle_primary_header() -> Result<Header, WriteError> {
    let mut header = Header::new();
    header
        .set(FORMAT_KEYWORD, BUNDLE_FORMAT)?
        .set(VERSION_KEYWORD, BUNDLE_VERSION)?;
    Ok(header)
}

/// A bundle being read: its file's HDUs, the path they were read from, and the load's context.
#[derive(Debug)]
struct BundleReader<'a> {
    reader: StreamReader<File>,
    path: &'a Path,
    ctx: &'a LoadCtx,
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
    fn idxs(&self) -> Result<BundleIdxs, BundleError> {
        // Each master finds its own flags when it is read; this refuses one that names none.
        flags_extension::check_claims(self.reader.hdus())
            .map_err(|error| BundleError::Flags(ImageError::fits_load(self.path, error)))?;
        let mut idxs = BundleIdxs::default();
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
                CalibrationComponent::Master(role) => idxs.masters.get_mut(role),
                CalibrationComponent::Defects => &mut idxs.defects,
            };
            if slot.replace(hdu).is_some() {
                return Err(BundleError::DuplicateExtension {
                    extname: extname.to_string(),
                });
            }
        }
        Ok(idxs)
    }

    /// The `role` master at HDU `idx`.
    fn master(&mut self, idx: usize, role: MasterRole) -> Result<CfaImage, BundleError> {
        let hdu = &self.reader.hdus()[idx];
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
        let ctx = self.ctx;
        let mut master = || {
            let selected = SelectedFits::lumos_cfa(&mut self.reader, idx, ctx)?;
            let cfa_type = selected.require_cfa()?;
            selected
                .read(ctx, Checksum::Verified(checksum), &SharedGains::default())?
                .into_cfa(cfa_type)
        };
        master().map_err(|source| match source {
            FitsLoadError::Cancelled => BundleError::Cancelled,
            source => BundleError::Master {
                role,
                source: ImageError::fits_load(self.path, source),
            },
        })
    }

    /// The defect map at HDU `idx`.
    fn defect_map(&mut self, idx: usize) -> Result<DefectMap, BundleError> {
        let header = &self.reader.hdus()[idx].header;
        if self.reader.hdus()[idx].kind != HduKind::BinTable
            || header.get_text(FORMAT_KEYWORD)? != Some(DEFECT_FORMAT)
            || header.get_integer(VERSION_KEYWORD)? != Some(BUNDLE_VERSION)
        {
            return Err(DefectTableError::Format.into());
        }
        let Some(dimensions) = read_defect_dimensions(header)? else {
            return Err(DefectTableError::Extent.into());
        };
        let table = self.reader.read_table(idx)?;
        let row_count = table.schema().nrows;
        let kinds: Vec<u8> = defect_column(&table, "KIND")?;
        let idxs: Vec<i64> = defect_column(&table, "INDEX")?;
        let excess: Vec<f32> = defect_column(&table, "EXCESS")?;
        if kinds.len() != row_count || idxs.len() != row_count || excess.len() != row_count {
            return Err(DefectTableError::ColLen.into());
        }
        let mut hot = HotPixels::default();
        let mut cold = Vec::new();
        for (row, ((kind, idx), excess)) in kinds.into_iter().zip(idxs).zip(excess).enumerate() {
            let Some(idx) = usize::try_from(idx)
                .ok()
                .filter(|&idx| idx < dimensions.pixel_count())
            else {
                return Err(DefectTableError::Idx { row }.into());
            };
            match kind {
                HOT if excess.is_finite() && excess > 0.0 => {
                    hot.idxs.push(idx);
                    hot.excess.push(excess);
                }
                COLD if excess.is_nan() => cold.push(idx),
                HOT | COLD => return Err(DefectTableError::Excess { row }.into()),
                kind => return Err(DefectTableError::Kind { row, kind }.into()),
            }
        }
        Ok(DefectMap::from_lists(dimensions, hot, cold).expect("every index lies on the sensor"))
    }
}

/// The values of the defect table's column `name`, refused unless it holds `T`.
fn defect_column<T>(table: &Table, name: &'static str) -> Result<Vec<T>, DefectTableError>
where
    Vec<T>: TryFrom<ColumnData>,
{
    let values = table.column(name).and_then(|col| col.raw());
    values
        .ok()
        .and_then(|values| Vec::try_from(values).ok())
        .ok_or(DefectTableError::Col { col: name })
}

#[derive(Debug)]
struct EncodedDefectMap {
    table: TableBuilder,
    header: Header,
}

fn encode_defect_map(map: &DefectMap) -> Result<EncodedDefectMap, BundleError> {
    let (hot, cold) = (map.hot(), map.cold_idxs());
    let rows = hot.idxs.len() + cold.len();
    let mut kinds = Vec::with_capacity(rows);
    kinds.resize(hot.idxs.len(), HOT);
    kinds.resize(rows, COLD);
    let mut excess = Vec::with_capacity(rows);
    excess.extend_from_slice(&hot.excess);
    excess.resize(rows, f32::NAN);
    let idxs = hot
        .idxs
        .iter()
        .chain(cold)
        .map(|&idx| i64::try_from(idx).map_err(|_| BundleError::DefectIdxOutOfRange { idx }))
        .collect::<Result<Vec<_>, _>>()?;
    let table = TableBuilder::with_columns(
        kinds.len(),
        [
            WriteColumn::scalar("KIND", ColumnData::U8(kinds)),
            WriteColumn::scalar("INDEX", ColumnData::I64(idxs)),
            WriteColumn::scalar("EXCESS", ColumnData::F32(excess)),
        ],
    )?;
    let mut header = Header::new();
    header
        .set("EXTNAME", CalibrationComponent::Defects.extname())?
        .set(FORMAT_KEYWORD, DEFECT_FORMAT)?
        .set(VERSION_KEYWORD, BUNDLE_VERSION)?;
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
    header.set("LUMWID", width)?.set("LUMHEI", height)?;
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
