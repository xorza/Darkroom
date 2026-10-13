//! [`OpenFits`]: a FITS file open for reading, with the path its refusals name.

use std::fs::File;
use std::ops::Range;
use std::path::Path;

use fits_well::image::Data as FitsData;
use fits_well::io::StreamReader;

use crate::image::cfa::cfa_image::CfaImage;
use crate::io::error::{FitsRefusal, ImageError};
use crate::io::fits::cfa;
use crate::io::fits::decode::hdu_sections::HduSections;
use crate::io::fits::decode::pixels::HduDecode;
use crate::io::fits::decode::plan::{DecodeLimit, FitsHduDescription};
use crate::io::fits::decode::selection;
use crate::io::fits::fits_checksum_provenance::FitsChecksumProvenance;
use crate::io::fits::fits_load_options::{FitsChecksumPolicy, FitsCubeInterpretation};
use crate::io::fits::flags_extension::FlagsExtension;
use crate::io::fits::hdu_list::HduList;
use crate::io::load_context::LoadContext;

/// A FITS file open for reading, and the path every refusal of it names.
#[derive(Debug)]
pub(crate) struct OpenFits<'a> {
    pub(crate) reader: &'a mut StreamReader<File>,
    pub(crate) path: &'a Path,
}

impl OpenFits<'_> {
    /// The Lumos CFA image in HDU `index`, with its flags extension, under the caller's
    /// `context` — its cancellation, memory limit and float scale — with the checksum state the
    /// caller verified for both. Held to the version check every file entry point applies; an HDU
    /// that is not a Lumos CFA image is refused.
    pub(crate) fn read_cfa_hdu(
        self,
        index: usize,
        context: &LoadContext,
        checksum: FitsChecksumProvenance,
    ) -> Result<CfaImage, ImageError> {
        let Self { reader, path } = self;
        context.check_cancelled(path)?;
        let hdu = &reader.hdus()[index];
        if !cfa::validate_image_header(path, &hdu.header)? {
            return Err(ImageError::fits_unsupported(
                path,
                FitsRefusal::NotLumosCfa { hdu: index },
            ));
        }
        let limit = DecodeLimit {
            path,
            bytes: context.memory_limit_bytes,
        };
        let plan = FitsHduDescription::from_hdu(path, hdu, index)?.plan(
            FitsCubeInterpretation::Reject,
            context.fits.float_scale,
            limit,
        )?;
        let size = plan.dimensions.size();
        let flags_hdu = FlagsExtension::locate(
            HduList {
                path,
                hdus: reader.hdus(),
            },
            index,
            size,
        )?;
        if flags_hdu.is_some() {
            plan.admit_flags_extension(limit)?;
        }
        let header = reader.hdus()[index].header.clone();
        let selected = selection::selected_hdu(
            HduList {
                path,
                hdus: reader.hdus(),
            },
            index,
        )?;
        let mut decoded = HduDecode {
            hdu: selected,
            header: &header,
            plan: &plan,
            path,
            context,
        }
        .read(&mut HduSections::new(reader, index), |_| Ok(checksum))?;
        if let Some(flags_hdu) = flags_hdu {
            context.check_cancelled(path)?;
            let stored = reader
                .read_image(flags_hdu)
                .map_err(|source| ImageError::fits(path, source))?
                .decode();
            decoded.flags = FlagsExtension::join(
                path,
                FlagsExtension::plane(stored, size),
                decoded.flags.as_ref(),
            )?;
        }
        decoded.into_cfa(path)
    }

    /// The samples of the extension at HDU `index` — flags or flat gain — over `section`, its whole
    /// shape, summed as they are read, and refused unless its checksum is valid.
    pub(super) fn read_extension(
        self,
        index: usize,
        section: &[Range<usize>],
    ) -> Result<FitsData, ImageError> {
        let Self { reader, path } = self;
        let fits = |source| ImageError::fits(path, source);
        let mut sum = reader.begin_data_checksum(index).map_err(fits)?;
        let stored = reader
            .read_image_section_summed(index, section, &mut sum)
            .map_err(fits)?
            .into_samples();
        let report = reader.finish_data_checksum(sum).map_err(fits)?;
        FitsChecksumPolicy::RequireValid.judge(report, index, path)?;
        Ok(stored)
    }
}
