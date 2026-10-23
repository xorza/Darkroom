//! [`SelectedFits`]: one image HDU of an open FITS file, selected and planned for a decode.

use std::fs::File;
use std::ops::Range;
use std::path::Path;

use arrayvec::ArrayVec;
use common::fs;
use fits_well::FitsReader;
use fits_well::header::Header;
use fits_well::image::View as FitsView;
use fits_well::io::StreamReader;

use crate::error::Cancelled;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::fits_checksum_provenance::FitsChecksumProvenance;
use crate::image::fits_transfer_provenance::FitsHduProvenance;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::io::fits::CHUNK_BYTES;
use crate::io::fits::decode::hdu_sections::HduSections;
use crate::io::fits::decode::pixels::HduDecode;
use crate::io::fits::decode::plan::{FitsDecodePlan, FitsHduDescription};
use crate::io::fits::decode::{DecodedFitsImage, selection};
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::fits_load_config::{FitsChecksumPolicy, FitsCubeInterpretation};
use crate::io::fits::flags_extension;
use crate::io::fits::gain_extension::{self, LocatedGain};
use crate::io::fits::lumos_extension::DescribedImage;
use crate::io::fits::{cfa, linear, metadata};
use crate::io::frame_info::{DecoderBytes, FrameInfo};
use crate::io::load_ctx::LoadCtx;

/// One image HDU of an open FITS file: the reader, the HDU, its decode plan, the mosaic its header
/// declares and the extensions beside it, each resolved once.
///
/// Every decode starts here, a file's and a calibration bundle's master alike, so each refuses the
/// same things in the same order: an HDU the options do not select, a Lumos CFA image of another
/// version, a header that declares no pattern lumos reads, then a size that overflows.
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
    /// The image's flat gain extension, when a flat divided it.
    gain: Option<LocatedGain>,
    /// The bytes of the gain its extension stores, as the decode holds it; none without one.
    pub(super) gain_bytes: usize,
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
pub(crate) fn open(path: &Path, ctx: &LoadCtx) -> Result<StreamReader<File>, FitsLoadError> {
    Cancelled::check(&ctx.cancel)?;
    let reader = FitsReader::open(fs::open(path)?)?;
    Cancelled::check(&ctx.cancel)?;
    Ok(reader)
}

impl<'r> SelectedFits<'r> {
    /// The image of `reader`, a file of its own, that the context's options select. A Lumos
    /// container of another format is refused first.
    pub(super) fn select(
        reader: &'r mut StreamReader<File>,
        ctx: &LoadCtx,
    ) -> Result<Self, FitsLoadError> {
        cfa::validate_container_format(reader.hdus().first().map(|hdu| &hdu.header))?;
        let selected = selection::select_image_hdu(reader.hdus(), &ctx.fits.hdu)?;
        Self::of(reader, selected, ctx.fits.cube, ctx)
    }

    /// The Lumos CFA image at HDU `idx` of `reader`, a calibration bundle; an HDU that is not
    /// one is refused.
    pub(crate) fn lumos_cfa(
        reader: &'r mut StreamReader<File>,
        idx: usize,
        ctx: &LoadCtx,
    ) -> Result<Self, FitsLoadError> {
        if !cfa::validate_image_header(&reader.hdus()[idx].header)? {
            return Err(FitsRefusal::NotLumosCfa { hdu: idx }.into());
        }
        let selected = selection::selected_hdu(reader.hdus(), idx)?;
        Self::of(reader, selected, FitsCubeInterpretation::Reject, ctx)
    }

    /// The image at the primary HDU of `reader`, whatever else the file holds: a stack product's.
    pub(crate) fn primary(
        reader: &'r mut StreamReader<File>,
        ctx: &LoadCtx,
    ) -> Result<Self, FitsLoadError> {
        let selected = selection::selected_hdu(reader.hdus(), 0)?;
        Self::of(reader, selected, ctx.fits.cube, ctx)
    }

    fn of(
        reader: &'r mut StreamReader<File>,
        selected: FitsHduProvenance,
        cube: FitsCubeInterpretation,
        ctx: &LoadCtx,
    ) -> Result<Self, FitsLoadError> {
        let hdu = &reader.hdus()[selected.idx];
        let lumos_cfa = cfa::validate_image_header(&hdu.header)?;
        let lumos_linear = linear::validate_image_header(&hdu.header)?;
        // A Lumos linear image states what its planes are: a cube of three is RGB.
        let cube = if lumos_linear {
            FitsCubeInterpretation::Rgb
        } else {
            cube
        };
        let plan =
            FitsHduDescription::from_hdu(hdu, selected.idx)?.plan(cube, ctx.fits.float_scale)?;
        let cfa_type = if plan.dimensions.is_grayscale() {
            metadata::read_cfa(
                &hdu.header,
                plan.dimensions.height(),
                ctx.fits.unstated_bayer_pattern,
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
        let flags_hdu = flags_extension::locate(reader.hdus(), selected.idx, &described)?;
        let gain = gain_extension::locate(reader.hdus(), selected.idx, &described)?;
        let mut extension_bytes = ArrayVec::new();
        if flags_hdu.is_some() {
            extension_bytes.push(plan.dimensions.pixel_count() as u64);
        }
        let gain_bytes = if let Some(gain) = gain {
            let image = reader.hdus()[gain.hdu].image()?;
            let nodes = image.shape.iter().product::<usize>();
            extension_bytes.push((nodes * image.bitpix.elem_size()) as u64);
            nodes * size_of::<f32>()
        } else {
            0
        };
        Ok(Self {
            reader,
            selected,
            plan,
            cfa_type,
            lumos_written: lumos_cfa || lumos_linear,
            flags_hdu,
            gain,
            gain_bytes,
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
        self.cfa_type.and_then(CfaType::mosaic).is_some()
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
            decoder: DecoderBytes {
                source: 0,
                beside: usize::try_from(self.beside_frame_bytes())
                    .expect("a decode the plan admitted fits the address space"),
            },
            gain_bytes: self.gain_bytes,
        })
    }

    /// What the decode holds beside the frame it makes — see
    /// [`FitsDecodePlan::beside_frame_bytes`] — over this file's extensions.
    pub(super) fn beside_frame_bytes(&self) -> u64 {
        self.plan.beside_frame_bytes(&self.extension_bytes)
    }

    fn header(&self) -> &Header {
        &self.reader.hdus()[self.selected.idx].header
    }

    /// Decode the selected image, its checksum settled as `checksum` says, then its flags and gain
    /// extensions, whose checksums are always required. The gain is the one `gains` holds of the
    /// key its extension states, read from no node, or the one it reads, shared through `gains`.
    pub(crate) fn read(
        self,
        ctx: &LoadCtx,
        checksum: Checksum,
        gains: &SharedGains,
    ) -> Result<DecodedFitsImage, FitsLoadError> {
        let size = self.plan.dimensions.size();
        let idx = self.selected.idx;
        let header = self.header().clone();
        let decode = HduDecode {
            hdu: self.selected,
            header: &header,
            plan: &self.plan,
            cfa_type: self.cfa_type,
            ctx,
        };
        let mut decoded = match checksum {
            Checksum::Judged => {
                let policy = if self.lumos_written {
                    FitsChecksumPolicy::RequireValid
                } else {
                    ctx.fits.checksum
                };
                decode.read_stream(self.reader, policy)?
            }
            Checksum::Verified(verified) => {
                decode.read(&mut HduSections::new(self.reader, idx), |_| Ok(verified))?
            }
        };
        if let Some(flags_hdu) = self.flags_hdu {
            Cancelled::check(&ctx.cancel)?;
            let bytes = read_extension::<u8>(self.reader, flags_hdu)?
                .pop()
                .expect("a flags extension is one plane");
            decoded.flags =
                flags_extension::join(flags_extension::plane(bytes, size), decoded.flags.as_ref())?;
        }
        if let Some(LocatedGain { hdu, key }) = self.gain {
            let shared = if let Some(shared) = key.and_then(|key| gains.get(key)) {
                shared
            } else {
                Cancelled::check(&ctx.cancel)?;
                let nodes = read_extension::<f32>(self.reader, hdu)?;
                gains.share(gain_extension::decode(nodes, size, key)?)
            };
            decoded.flat_gain = Some(FrameGain::Shared(shared));
        }
        Ok(decoded)
    }
}

/// A sample type an extension stores: the bytes of a flags plane or a mask, or the `f32`s of a
/// flat gain or a quality plane.
pub(crate) trait ExtensionSample: Copy {
    /// The samples of `view`, of the `BITPIX` the extension's check admitted.
    fn of(view: FitsView<'_>) -> &[Self];
}

impl ExtensionSample for u8 {
    fn of(view: FitsView<'_>) -> &[Self] {
        let FitsView::U8(samples) = view else {
            unreachable!("the extension's check admitted only BITPIX = 8");
        };
        samples
    }
}

impl ExtensionSample for f32 {
    fn of(view: FitsView<'_>) -> &[Self] {
        let FitsView::F32(samples) = view else {
            unreachable!("the extension's check admitted only BITPIX = -32");
        };
        samples
    }
}

/// The planes of the extension at HDU `idx` of `reader` — flags, a flat gain, a mask or a
/// quality plane — one per slice of its third axis, or one for an image of two: each read a chunk
/// of rows at a time straight into its own plane, summed as it is read, and refused unless the
/// checksum is valid. No whole-unit buffer is read and copied out.
pub(crate) fn read_extension<T: ExtensionSample>(
    reader: &mut StreamReader<File>,
    idx: usize,
) -> Result<ArrayVec<Vec<T>, 3>, FitsLoadError> {
    read_extension_in_chunks(reader, idx, CHUNK_BYTES)
}

/// [`read_extension`], a chunk of rows of at most `chunk_bytes` at a time, one row at least.
fn read_extension_in_chunks<T: ExtensionSample>(
    reader: &mut StreamReader<File>,
    idx: usize,
    chunk_bytes: usize,
) -> Result<ArrayVec<Vec<T>, 3>, FitsLoadError> {
    let shape = ArrayVec::<usize, 3>::try_from(reader.hdus()[idx].image()?.shape)
        .expect("the extension's check admitted at most three axes");
    debug_assert!(shape.len() >= 2, "an extension is a plane at least");
    let (width, height) = (shape[0], shape[1]);
    let rows_per_chunk = (chunk_bytes / (width * size_of::<T>()).max(1)).max(1);
    let mut sections = HduSections::summed(reader, idx)?;
    let mut planes = ArrayVec::new();
    for slice in 0..shape.get(2).copied().unwrap_or(1) {
        let mut plane = Vec::with_capacity(width * height);
        for start in (0..height).step_by(rows_per_chunk) {
            let mut ranges: ArrayVec<Range<usize>, 3> =
                [0..width, start..(start + rows_per_chunk).min(height)]
                    .into_iter()
                    .collect();
            if shape.len() == 3 {
                ranges.push(slice..slice + 1);
            }
            plane.extend_from_slice(T::of(sections.read_section(&ranges)?.samples()));
        }
        planes.push(plane);
    }
    let report = sections.finish_checksum()?;
    FitsChecksumPolicy::RequireValid.judge(report, idx)?;
    Ok(planes)
}

#[cfg(test)]
mod tests {
    use common::{CancelToken, TempDir, fs};
    use fits_well::FitsWriter;
    use fits_well::image::Image;
    use fits_well::io::{BLOCK_SIZE, ChecksumReport, ChecksumStatus};

    use crate::internals::fits::fits_refusal;
    use crate::io::fits::decode::selected_fits::{self, read_extension_in_chunks};
    use crate::io::fits::error::FitsRefusal;
    use crate::io::load_ctx::LoadCtx;

    /// An extension is read a chunk of rows at a time straight into a plane per slice, and its
    /// checksum is summed across every chunk of every slice. A 3 × 5 × 3 cube of `f32`, 12 B a
    /// row, read 24 B at a time: two rows, two rows, one, in each of the three slices, which come
    /// back as written; a 3 × 5 byte plane read a row at a time is one plane. A flipped bit in the
    /// last slice is refused, as every extension requires a valid checksum.
    #[test]
    fn an_extension_reads_in_chunks_into_a_plane_per_slice() {
        let dir = TempDir::new("read_extension_chunks");
        let ctx = LoadCtx::new(CancelToken::never());
        let samples: Vec<f32> = (0..45).map(|idx| idx as f32).collect();
        let bytes: Vec<u8> = (0..15).collect();
        let path = dir.join("extension.fits");
        let mut writer = FitsWriter::new(fs::create(&path).unwrap()).with_checksums();
        writer
            .write_image(&Image::new([3, 5, 3], samples.clone()).unwrap(), None)
            .unwrap();
        writer
            .write_image(&Image::new([3, 5], bytes.clone()).unwrap(), None)
            .unwrap();
        drop(writer);

        let mut reader = selected_fits::open(&path, &ctx).unwrap();
        let planes = read_extension_in_chunks::<f32>(&mut reader, 0, 24).unwrap();
        assert_eq!(planes.len(), 3);
        for (slice, plane) in planes.iter().enumerate() {
            assert_eq!(plane, &samples[slice * 15..(slice + 1) * 15]);
        }
        let planes = read_extension_in_chunks::<u8>(&mut reader, 1, 1).unwrap();
        assert_eq!(planes.as_slice(), [bytes]);

        let mut corrupt = fs::read(&path).unwrap();
        // The last sample of the cube, 44.0, ends the first data block's 180 bytes.
        corrupt[BLOCK_SIZE + 179] ^= 0x01;
        fs::write(&path, corrupt).unwrap();
        let mut reader = selected_fits::open(&path, &ctx).unwrap();
        assert_eq!(
            fits_refusal(read_extension_in_chunks::<f32>(&mut reader, 0, 24).unwrap_err()),
            FitsRefusal::ChecksumRequired {
                hdu: 0,
                report: ChecksumReport {
                    datasum: ChecksumStatus::Invalid,
                    checksum: ChecksumStatus::Invalid,
                },
            }
        );
    }
}
