//! [`SelectedFits`]: a FITS file opened for the one image a load reads from it.

use std::fs::File;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use arrayvec::ArrayVec;
use fits_well::FitsReader;
use fits_well::header::Header;
use fits_well::io::StreamReader;

use crate::io::error::ImageError;
use crate::io::fits::cfa;
use crate::io::fits::decode::DecodedFitsImage;
use crate::io::fits::decode::open_fits::OpenFits;
use crate::io::fits::decode::pixels::HduDecode;
use crate::io::fits::decode::plan::{DecodeLimit, FitsDecodePlan, FitsHduDescription};
use crate::io::fits::decode::selection;
use crate::io::fits::fits_load_options::FitsChecksumPolicy;
use crate::io::fits::fits_transfer_provenance::FitsHduProvenance;
use crate::io::fits::flags_extension::FlagsExtension;
use crate::io::fits::gain_extension::GainExtension;
use crate::io::fits::hdu_list::HduList;
use crate::io::load_context::LoadContext;

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
    /// The bytes of each extension the decode reads after the image, in the order it reads them:
    /// the flags, a byte a pixel, then the gain grid.
    extension_bytes: ArrayVec<u64, 2>,
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
        cfa::validate_container_format(path, reader.hdus().first().map(|hdu| &hdu.header))?;
        let selected = selection::select_image_hdu(
            HduList {
                path,
                hdus: reader.hdus(),
            },
            &context.fits.hdu,
        )?;
        let hdu = &reader.hdus()[selected.index];
        let lumos_cfa = cfa::validate_image_header(path, &hdu.header)?;
        let limit = DecodeLimit {
            path,
            bytes: context.memory_limit_bytes,
        };
        let plan = FitsHduDescription::from_hdu(path, hdu, selected.index)?.plan(
            context.fits.cube,
            context.fits.float_scale,
            limit,
        )?;
        let flags_hdu = FlagsExtension::locate(
            HduList {
                path,
                hdus: reader.hdus(),
            },
            selected.index,
            plan.dimensions.size(),
        )?;
        if flags_hdu.is_some() {
            plan.admit_flags_extension(limit)?;
        }
        let gain_hdu = GainExtension::locate(
            HduList {
                path,
                hdus: reader.hdus(),
            },
            selected.index,
            plan.dimensions.size(),
        )?;
        let mut extension_bytes = ArrayVec::new();
        if flags_hdu.is_some() {
            extension_bytes.push(plan.dimensions.pixel_count() as u64);
        }
        if let Some(gain_hdu) = gain_hdu {
            let image = reader.hdus()[gain_hdu]
                .image()
                .map_err(|source| ImageError::fits(path, source))?;
            extension_bytes
                .push((image.shape.iter().product::<usize>() * image.bitpix.elem_size()) as u64);
        }
        Ok(Self {
            reader,
            selected,
            plan,
            lumos_cfa,
            flags_hdu,
            gain_hdu,
            extension_bytes,
        })
    }

    /// What the decode holds beside the frame it makes — see
    /// [`FitsDecodePlan::beside_frame_bytes`] — over this file's extensions.
    pub(super) fn beside_frame_bytes(&self) -> u64 {
        self.plan.beside_frame_bytes(&self.extension_bytes)
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
        let header = self.header().clone();
        let mut decoded = HduDecode {
            hdu: self.selected,
            header: &header,
            plan: &self.plan,
            path,
            context,
        }
        .read_stream(&mut self.reader, policy)?;
        if let Some(flags_hdu) = self.flags_hdu {
            context.check_cancelled(path)?;
            let stored = OpenFits {
                reader: &mut self.reader,
                path,
            }
            .read_extension(flags_hdu, &[0..size.width, 0..size.height])?;
            decoded.flags = FlagsExtension::join(
                path,
                FlagsExtension::plane(stored, size),
                decoded.flags.as_ref(),
            )?;
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
            let stored = OpenFits {
                reader: &mut self.reader,
                path,
            }
            .read_extension(gain_hdu, &shape)?;
            decoded.metadata.flat_gain = Some(Arc::new(GainExtension::decode(path, stored, size)?));
        }
        Ok(decoded)
    }
}
