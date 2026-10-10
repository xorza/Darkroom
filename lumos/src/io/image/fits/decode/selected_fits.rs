//! [`SelectedFits`]: a FITS file opened for the one image a load reads from it.

use std::fs::File;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use fits_well::FitsReader;
use fits_well::header::Header;
use fits_well::image::ImageData;
use fits_well::io::StreamReader;

use crate::io::image::error::ImageError;
use crate::io::image::fits::cfa::{validate_cfa_container_format, validate_cfa_image_header};
use crate::io::image::fits::decode::DecodedFitsImage;
use crate::io::image::fits::decode::pixels;
use crate::io::image::fits::decode::plan::{
    FitsDecodePlan, FitsHduDescription, preflight_fits_image,
};
use crate::io::image::fits::decode::selection;
use crate::io::image::fits::flags_extension::FlagsExtension;
use crate::io::image::fits::gain_extension::GainExtension;
use crate::io::image::fits::options::FitsChecksumPolicy;
use crate::io::image::fits::provenance::FitsHduProvenance;
use crate::io::image::load_context::LoadContext;

/// A FITS file opened for one image: the reader, the HDU the options select, and that HDU's
/// decode plan.
///
/// Every file entry point starts here, so each refuses the same things in the same order: a
/// Lumos container of another format, then an HDU the options do not select, then a Lumos CFA
/// image of another version, then a decode the memory limit cannot hold.
#[derive(Debug)]
pub(super) struct SelectedFits {
    reader: StreamReader<File>,
    pub(super) selected: FitsHduProvenance,
    pub(super) plan: FitsDecodePlan,
    /// Whether the image is one Lumos wrote, whose checksum is then required to be valid.
    lumos_cfa: bool,
    /// The HDU of the image's flags extension, when it has one.
    flags_hdu: Option<usize>,
    /// The HDU of the image's flat gain extension, when a flat divided it.
    gain_hdu: Option<usize>,
}

impl SelectedFits {
    pub(super) fn open(path: &Path, context: &LoadContext) -> Result<Self, ImageError> {
        context.check_cancelled(path)?;
        let file = File::open(path).map_err(|source| ImageError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let reader = FitsReader::open(file).map_err(|source| ImageError::fits(path, source))?;
        context.check_cancelled(path)?;
        validate_cfa_container_format(path, reader.hdus().first().map(|hdu| &hdu.header))?;
        let selected = selection::select_image_hdu(path, reader.hdus(), &context.fits.hdu)?;
        let hdu = &reader.hdus()[selected.index];
        let lumos_cfa = validate_cfa_image_header(path, &hdu.header)?;
        let plan = preflight_fits_image(
            path,
            FitsHduDescription::from_hdu(path, hdu)?,
            context.fits.cube,
            context.fits.float_scale,
            context.memory_limit_bytes,
        )?;
        let flags_hdu =
            FlagsExtension::locate(path, reader.hdus(), selected.index, plan.dimensions.size())?;
        if flags_hdu.is_some() {
            plan.admit_flags_extension(path, context.memory_limit_bytes)?;
        }
        let gain_hdu =
            GainExtension::locate(path, reader.hdus(), selected.index, plan.dimensions.size())?;
        Ok(Self {
            reader,
            selected,
            plan,
            lumos_cfa,
            flags_hdu,
            gain_hdu,
        })
    }

    /// The selected HDU's header.
    pub(super) fn header(&self) -> &Header {
        &self.reader.hdus()[self.selected.index].header
    }

    /// Decode the selected image, with the checksum the options ask for — or, for an image Lumos
    /// wrote, require it — summed from the bytes the decode reads, and its flags extension, whose
    /// checksum is always required.
    pub(super) fn read(
        mut self,
        path: &Path,
        context: &LoadContext,
    ) -> Result<DecodedFitsImage, ImageError> {
        let policy = if self.lumos_cfa {
            FitsChecksumPolicy::RequireValid
        } else {
            context.fits.checksum
        };
        let size = self.plan.dimensions.size();
        let mut decoded = pixels::read_stream_hdu(
            &mut self.reader,
            self.selected,
            policy,
            path,
            &self.plan,
            context,
        )?;
        if let Some(flags_hdu) = self.flags_hdu {
            context.check_cancelled(path)?;
            let stored = read_extension(
                &mut self.reader,
                flags_hdu,
                &[0..size.width, 0..size.height],
                path,
            )?;
            decoded.flags = FlagsExtension::join(path, stored, size, decoded.flags.as_ref())?;
        }
        if let Some(gain_hdu) = self.gain_hdu {
            context.check_cancelled(path)?;
            let shape: Vec<Range<usize>> = self.reader.hdus()[gain_hdu]
                .image()
                .map_err(|source| ImageError::fits(path, source))?
                .shape
                .iter()
                .map(|&extent| 0..extent)
                .collect();
            let stored = read_extension(&mut self.reader, gain_hdu, &shape, path)?;
            decoded.metadata.flat_gain = Some(Arc::new(GainExtension::decode(path, stored, size)?));
        }
        Ok(decoded)
    }
}

/// The samples of the extension at HDU `index` — flags or flat gain — over `section`, its whole
/// shape, summed as they are read, and refused unless its checksum is valid.
fn read_extension(
    reader: &mut StreamReader<File>,
    index: usize,
    section: &[Range<usize>],
    path: &Path,
) -> Result<ImageData, ImageError> {
    let fits = |source| ImageError::fits(path, source);
    let mut sum = reader.begin_data_checksum(index).map_err(fits)?;
    let stored = reader
        .read_image_section_summed(index, section, &mut sum)
        .map_err(fits)?
        .into_samples();
    let report = reader.finish_data_checksum(sum).map_err(fits)?;
    selection::judge_checksum(report, index, path, FitsChecksumPolicy::RequireValid)?;
    Ok(stored)
}
