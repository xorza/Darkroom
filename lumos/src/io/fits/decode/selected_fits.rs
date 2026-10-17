//! [`SelectedFits`]: one image HDU of an open FITS file, selected and planned for a decode.

use std::fs::File;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use arrayvec::ArrayVec;
use fits_well::FitsReader;
use fits_well::header::Header;
use fits_well::image::Data as FitsData;
use fits_well::io::StreamReader;

use crate::error::Cancelled;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::fits_checksum_provenance::FitsChecksumProvenance;
use crate::image::fits_transfer_provenance::FitsHduProvenance;
use crate::io::fits::decode::hdu_sections::HduSections;
use crate::io::fits::decode::pixels::HduDecode;
use crate::io::fits::decode::plan::{FitsDecodePlan, FitsHduDescription};
use crate::io::fits::decode::{DecodedFitsImage, selection};
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::fits_load_options::{FitsChecksumPolicy, FitsCubeInterpretation};
use crate::io::fits::flags_extension::FlagsExtension;
use crate::io::fits::gain_extension::GainExtension;
use crate::io::fits::lumos_extension::DescribedImage;
use crate::io::fits::{cfa, linear, metadata};
use crate::io::frame_info::FrameInfo;
use crate::io::load_context::LoadContext;

/// One image HDU of an open FITS file: the reader, the HDU, its decode plan, the mosaic its header
/// declares and the extensions beside it, each resolved once.
///
/// Every decode starts here, a file's and a calibration bundle's master alike, so each refuses the
/// same things in the same order: an HDU the options do not select, a Lumos CFA image of another
/// version, a header that declares no pattern lumos reads, then a decode the memory limit cannot
/// hold.
#[derive(Debug)]
pub(crate) struct SelectedFits<'r> {
    reader: &'r mut StreamReader<File>,
    pub(super) selected: FitsHduProvenance,
    pub(super) plan: FitsDecodePlan,
    /// The pattern the header declares for a single-plane image; a cube's mosaic keywords describe
    /// the frame before someone else's demosaic, so they say nothing about its planes.
    pub(super) cfa_type: Option<CfaType>,
    /// Whether the image is one Lumos wrote, whose checksum is then required to be valid.
    lumos_written: bool,
    /// The HDU of the image's flags extension, when it has one.
    flags_hdu: Option<usize>,
    /// The HDU of the image's flat gain extension, when a flat divided it.
    gain_hdu: Option<usize>,
    /// The bytes of each extension the decode reads after the image, in the order it reads them:
    /// the flags, a byte a pixel, then the gain grid.
    extension_bytes: ArrayVec<u64, 2>,
}

/// How a decode settles the selected image's checksum.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Checksum {
    /// Judged under the load's policy from the bytes the decode reads; an image Lumos wrote
    /// requires a valid one.
    Judged,
    /// Verified before the decode, as a calibration bundle verifies every HDU it holds.
    Verified(FitsChecksumProvenance),
}

/// The FITS file at `path`, open for reading.
pub(crate) fn open(
    path: &Path,
    context: &LoadContext,
) -> Result<StreamReader<File>, FitsLoadError> {
    Cancelled::check(&context.cancel)?;
    let reader = FitsReader::open(File::open(path)?)?;
    Cancelled::check(&context.cancel)?;
    Ok(reader)
}

impl<'r> SelectedFits<'r> {
    /// The image of `reader`, a file of its own, that the context's options select. A Lumos
    /// container of another format is refused first.
    pub(super) fn select(
        reader: &'r mut StreamReader<File>,
        context: &LoadContext,
    ) -> Result<Self, FitsLoadError> {
        cfa::validate_container_format(reader.hdus().first().map(|hdu| &hdu.header))?;
        let selected = selection::select_image_hdu(reader.hdus(), &context.fits.hdu)?;
        Self::of(reader, selected, context.fits.cube, context)
    }

    /// The Lumos CFA image at HDU `index` of `reader`, a calibration bundle; an HDU that is not
    /// one is refused.
    pub(crate) fn lumos_cfa(
        reader: &'r mut StreamReader<File>,
        index: usize,
        context: &LoadContext,
    ) -> Result<Self, FitsLoadError> {
        if !cfa::validate_image_header(&reader.hdus()[index].header)? {
            return Err(FitsRefusal::NotLumosCfa { hdu: index }.into());
        }
        let selected = selection::selected_hdu(reader.hdus(), index)?;
        Self::of(reader, selected, FitsCubeInterpretation::Reject, context)
    }

    /// The image at the primary HDU of `reader`, whatever else the file holds: a stack product's.
    pub(crate) fn primary(
        reader: &'r mut StreamReader<File>,
        context: &LoadContext,
    ) -> Result<Self, FitsLoadError> {
        let selected = selection::selected_hdu(reader.hdus(), 0)?;
        Self::of(reader, selected, context.fits.cube, context)
    }

    fn of(
        reader: &'r mut StreamReader<File>,
        selected: FitsHduProvenance,
        cube: FitsCubeInterpretation,
        context: &LoadContext,
    ) -> Result<Self, FitsLoadError> {
        let hdu = &reader.hdus()[selected.index];
        let lumos_cfa = cfa::validate_image_header(&hdu.header)?;
        let lumos_linear = linear::validate_image_header(&hdu.header)?;
        // A Lumos linear image states what its planes are: a cube of three is RGB.
        let cube = if lumos_linear {
            FitsCubeInterpretation::Rgb
        } else {
            cube
        };
        let limit = context.memory_limit_bytes;
        let plan = FitsHduDescription::from_hdu(hdu, selected.index)?.plan(
            cube,
            context.fits.float_scale,
            limit,
        )?;
        let cfa_type = if plan.dimensions.is_grayscale() {
            metadata::read_cfa(
                &hdu.header,
                plan.dimensions.height(),
                context.fits.unstated_bayer_pattern,
            )?
        } else {
            None
        };
        let size = plan.dimensions.size();
        let described = DescribedImage {
            size,
            channels: plan.dimensions.channels(),
            scale: Some(plan.sample_scale.divisor),
            weight_kind: None,
        };
        let flags_hdu = FlagsExtension::locate(reader.hdus(), selected.index, &described)?;
        if flags_hdu.is_some() {
            plan.admit_flags_extension(limit)?;
        }
        let gain_hdu = GainExtension::locate(reader.hdus(), selected.index, &described)?;
        let mut extension_bytes = ArrayVec::new();
        if flags_hdu.is_some() {
            extension_bytes.push(plan.dimensions.pixel_count() as u64);
        }
        if let Some(gain_hdu) = gain_hdu {
            let image = reader.hdus()[gain_hdu].image()?;
            extension_bytes
                .push((image.shape.iter().product::<usize>() * image.bitpix.elem_size()) as u64);
        }
        Ok(Self {
            reader,
            selected,
            plan,
            cfa_type,
            lumos_written: lumos_cfa || lumos_linear,
            flags_hdu,
            gain_hdu,
            extension_bytes,
        })
    }

    /// The pattern the header declares for a single-plane image.
    pub(crate) const fn cfa_type(&self) -> Option<CfaType> {
        self.cfa_type
    }

    /// Whether the header declares a mosaic, which loads as a `CfaImage`: a mono sensor frame has
    /// none to demosaic, so it is linear data like any other single-plane image.
    pub(super) fn is_mosaic(&self) -> bool {
        self.cfa_type
            .is_some_and(|cfa_type| cfa_type != CfaType::Mono)
    }

    /// The pattern of a sensor frame, which a CFA load requires: one plane, and a pattern its
    /// header declares.
    pub(crate) fn require_cfa(&self) -> Result<CfaType, FitsLoadError> {
        let dimensions = self.plan.dimensions;
        if !dimensions.is_grayscale() {
            return Err(FitsRefusal::CfaPlanes {
                planes: dimensions.channels(),
            }
            .into());
        }
        self.cfa_type
            .ok_or_else(|| FitsRefusal::MissingCfaPattern.into())
    }

    /// The sensor frame as its header states it, which a CFA load requires ([`Self::require_cfa`]).
    pub(crate) fn cfa_frame_info(&self) -> Result<FrameInfo, FitsLoadError> {
        Ok(FrameInfo {
            dimensions: self.plan.dimensions,
            cfa_type: self.require_cfa()?,
            decoder_bytes: usize::try_from(self.beside_frame_bytes())
                .expect("a decode the plan admitted fits the address space"),
        })
    }

    /// What the decode holds beside the frame it makes — see
    /// [`FitsDecodePlan::beside_frame_bytes`] — over this file's extensions.
    pub(super) fn beside_frame_bytes(&self) -> u64 {
        self.plan.beside_frame_bytes(&self.extension_bytes)
    }

    fn header(&self) -> &Header {
        &self.reader.hdus()[self.selected.index].header
    }

    /// Decode the selected image, its checksum settled as `checksum` says, then its flags and gain
    /// extensions, whose checksums are always required.
    pub(crate) fn read(
        self,
        context: &LoadContext,
        checksum: Checksum,
    ) -> Result<DecodedFitsImage, FitsLoadError> {
        let size = self.plan.dimensions.size();
        let index = self.selected.index;
        let header = self.header().clone();
        let decode = HduDecode {
            hdu: self.selected,
            header: &header,
            plan: &self.plan,
            cfa_type: self.cfa_type,
            context,
        };
        let mut decoded = match checksum {
            Checksum::Judged => {
                let policy = if self.lumos_written {
                    FitsChecksumPolicy::RequireValid
                } else {
                    context.fits.checksum
                };
                decode.read_stream(self.reader, policy)?
            }
            Checksum::Verified(verified) => {
                decode.read(&mut HduSections::new(self.reader, index), |_| Ok(verified))?
            }
        };
        if let Some(flags_hdu) = self.flags_hdu {
            Cancelled::check(&context.cancel)?;
            let stored = read_extension(self.reader, flags_hdu, &[0..size.width, 0..size.height])?;
            decoded.flags =
                FlagsExtension::join(FlagsExtension::plane(stored, size), decoded.flags.as_ref())?;
        }
        if let Some(gain_hdu) = self.gain_hdu {
            Cancelled::check(&context.cancel)?;
            let shape: Vec<Range<usize>> = self.reader.hdus()[gain_hdu]
                .image()?
                .shape
                .iter()
                .map(|&extent| 0..extent)
                .collect();
            let stored = read_extension(self.reader, gain_hdu, &shape)?;
            decoded.flat_gain = Some(Arc::new(GainExtension::decode(stored, size)?));
        }
        Ok(decoded)
    }
}

/// The samples of the extension at HDU `index` of `reader` — flags or flat gain — over
/// `section`, its whole shape, summed as they are read, and refused unless its checksum is valid.
pub(crate) fn read_extension(
    reader: &mut StreamReader<File>,
    index: usize,
    section: &[Range<usize>],
) -> Result<FitsData, FitsLoadError> {
    let mut sum = reader.begin_data_checksum(index)?;
    let stored = reader
        .read_image_section_summed(index, section, &mut sum)?
        .into_samples();
    let report = reader.finish_data_checksum(sum)?;
    FitsChecksumPolicy::RequireValid.judge(report, index)?;
    Ok(stored)
}
