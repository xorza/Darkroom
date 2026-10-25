//! [`SelectedFits`]: one image HDU of an open FITS file, selected and planned for a decode.

use std::fs::File;
use std::path::Path;

use arrayvec::ArrayVec;
use common::fs;
use fits_well::error::ExpectedHdu;
use fits_well::image::View as FitsView;
use fits_well::io::StreamReader;
use fits_well::{FitsError, FitsReader};

use crate::error::Cancelled;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::metadata::fits_checksum_provenance::FitsChecksumProvenance;
use crate::image::metadata::fits_transfer_provenance::FitsHduProvenance;
use crate::io::fits::CHUNK_BYTES;
use crate::io::fits::decode::extension_chunks::ExtensionChunks;
use crate::io::fits::decode::pixels::HduDecode;
use crate::io::fits::decode::plan::{self, FitsDecodePlan, FitsHduDescription};
use crate::io::fits::decode::{DecodedFitsImage, selection};
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::fits_load_config::{FitsChecksumPolicy, FitsCubeInterpretation};
use crate::io::fits::flags_extension;
use crate::io::fits::gain_extension::{self, LocatedGain};
use crate::io::fits::lumos_extension::{DescribedImage, WeightUnits};
use crate::io::fits::tile_parallel;
use crate::io::fits::{cfa, linear, metadata};
use crate::io::frame_info::{DecoderBytes, FrameInfo};
use crate::io::load_ctx::LoadCtx;

/// One image HDU of an open FITS file: the HDU, its decode plan, the mosaic its header declares
/// and the extensions beside it, each resolved once, for a read of the reader they came from.
///
/// Every decode starts here, a file's and a calibration bundle's master alike, so each refuses the
/// same things in the same order: an HDU the options do not select, a Lumos CFA image of another
/// version, a header that declares no pattern lumos reads, then a size that overflows.
#[derive(Debug)]
pub(crate) struct SelectedFits {
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
    /// What reading each extension after the image holds, in the order the decode reads them:
    /// the flags, then the gain grid.
    extension_bytes: ArrayVec<u64, 2>,
    /// The heap the reader keeps for the image from the selection to the decode: a compressed
    /// image's tile index.
    pub(super) held_bytes: usize,
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

impl SelectedFits {
    /// The image of `reader`, a file of its own, that the context's options select. A Lumos
    /// container of another format is refused first.
    pub(super) fn select(
        reader: &mut StreamReader<File>,
        ctx: &LoadCtx,
    ) -> Result<Self, FitsLoadError> {
        cfa::validate_container_format(reader.hdus().first().map(|hdu| &hdu.header))?;
        let selected = selection::select_image_hdu(reader.hdus(), &ctx.fits.hdu)?;
        Self::of(reader, selected, ctx.fits.cube, ctx)
    }

    /// The Lumos CFA image at HDU `idx` of `reader`, a calibration bundle; an HDU that is not
    /// one is refused.
    pub(crate) fn lumos_cfa(
        reader: &mut StreamReader<File>,
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
        reader: &mut StreamReader<File>,
        ctx: &LoadCtx,
    ) -> Result<Self, FitsLoadError> {
        let selected = selection::selected_hdu(reader.hdus(), 0)?;
        Self::of(reader, selected, ctx.fits.cube, ctx)
    }

    fn of(
        reader: &mut StreamReader<File>,
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
        let source_bytes = hdu.padded_data_bytes();
        // The plan sizes the reads from the handle, which a compressed image's tile index opens.
        let image = reader
            .image(selected.idx)
            .map_err(|source| not_an_image(source, selected.idx))?;
        let plan = FitsHduDescription::of_image(&image, source_bytes).plan(
            cube,
            ctx.fits.float_scale,
            image.tiles(),
            |regions| plan::reader_bytes(&image, regions),
        )?;
        let held_bytes = usize::try_from(image.view_bytes(std::iter::empty())?.kept)
            .expect("a tile index read fits the address space");
        let hdu = &reader.hdus()[selected.idx];
        let cfa_type = if plan.desc.is_mono() {
            metadata::read_cfa(
                &hdu.header,
                plan.desc.height(),
                ctx.fits.unstated_bayer_pattern,
            )?
        } else {
            None
        };
        let size = plan.desc.size();
        let described = DescribedImage {
            size,
            channels: plan.desc.channel_count(),
            scale: Some(plan.sample_scale.divisor),
            weight_units: WeightUnits::Unitless,
        };
        let flags_hdu = flags_extension::locate(reader.hdus(), selected.idx, &described)?;
        let gain = gain_extension::locate(reader.hdus(), selected.idx, &described)?;
        let mut extension_bytes = ArrayVec::new();
        if let Some(flags_hdu) = flags_hdu {
            extension_bytes.push(extension_read_bytes(reader, flags_hdu)?);
        }
        let gain_bytes = if let Some(gain) = gain {
            let nodes = reader.hdus()[gain.hdu]
                .image_metadata()?
                .shape
                .iter()
                .product::<usize>();
            extension_bytes.push(extension_read_bytes(reader, gain.hdu)?);
            nodes * size_of::<f32>()
        } else {
            0
        };
        Ok(Self {
            selected,
            plan,
            cfa_type,
            lumos_written: lumos_cfa || lumos_linear,
            flags_hdu,
            gain,
            gain_bytes,
            extension_bytes,
            held_bytes,
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
        let desc = self.plan.desc;
        if !desc.is_mono() {
            return Err(FitsRefusal::CfaPlanes {
                planes: desc.channel_count(),
            }
            .into());
        }
        self.cfa_type
            .ok_or_else(|| FitsRefusal::MissingCfaPattern.into())
    }

    /// The sensor frame as its header states it, which a CFA load requires ([`Self::require_cfa`]).
    pub(super) fn cfa_frame_info(&self) -> Result<FrameInfo, FitsLoadError> {
        Ok(self.frame_info(self.require_cfa()?))
    }

    /// The image as a linear load reads it, whose header declares no mosaic.
    pub(super) fn linear_frame_info(&self) -> FrameInfo {
        debug_assert!(!self.is_mosaic(), "a linear load refuses a mosaic");
        self.frame_info(CfaType::Mono)
    }

    /// The frame as `cfa_type`: its dimensions, what its decode holds beside it over this file's
    /// extensions — see [`FitsDecodePlan::beside_frame_bytes`] — and the gain it reads.
    fn frame_info(&self, cfa_type: CfaType) -> FrameInfo {
        FrameInfo {
            desc: self.plan.desc,
            cfa_type,
            decoder: DecoderBytes {
                source: 0,
                beside: usize::try_from(self.plan.beside_frame_bytes(&self.extension_bytes))
                    .expect("a decode the plan admitted fits the address space"),
            },
            gain_bytes: self.gain_bytes,
        }
    }

    /// Decode the selected image of `reader`, the reader it was selected from, its checksum
    /// settled as `checksum` says, then its flags and gain extensions, whose checksums are always
    /// required. The gain is the one `gains` holds of the key its extension states, read from no
    /// node, or the one it reads, shared through `gains`.
    pub(crate) fn read(
        self,
        reader: &mut StreamReader<File>,
        ctx: &LoadCtx,
        checksum: Checksum,
        gains: &SharedGains,
    ) -> Result<DecodedFitsImage, FitsLoadError> {
        let size = self.plan.desc.size();
        let image = reader.image(self.selected.idx)?;
        let decode = HduDecode {
            hdu: self.selected,
            header: image.header(),
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
                decode.read_stream(image, policy)?
            }
            Checksum::Verified(verified) => decode.read(image, |_| Ok(verified))?,
        };
        if let Some(flags_hdu) = self.flags_hdu {
            Cancelled::check(&ctx.cancel)?;
            let bytes = read_extension::<u8>(reader, flags_hdu)?
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
                let nodes = read_extension::<f32>(reader, hdu)?;
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
        view.as_u8()
            .expect("the extension's check admitted only BITPIX = 8")
    }
}

impl ExtensionSample for f32 {
    fn of(view: FitsView<'_>) -> &[Self] {
        view.as_f32()
            .expect("the extension's check admitted only BITPIX = -32")
    }
}

/// The planes of the extension at HDU `idx` of `reader` — flags, a flat gain, a mask or a
/// quality plane — one per slice of its third axis, or one for an image of two: read a chunk of
/// rows of every plane at a time — see [`ExtensionChunks`] — summed as it is read, and refused
/// unless the checksum is valid. No whole-unit buffer is read and copied out.
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
    let chunks = ExtensionChunks::of(
        reader.hdus()[idx].image_metadata()?.shape,
        size_of::<T>(),
        chunk_bytes,
    );
    let mut image = reader.image(idx)?.with_checksums();
    let mut words = Vec::new();
    let mut planes: ArrayVec<Vec<T>, 3> = (0..chunks.planes())
        .map(|_| Vec::with_capacity(chunks.plane_len()))
        .collect();
    for ranges in chunks.regions() {
        let samples =
            T::of(tile_parallel::view(&mut image, ranges.as_slice(), &mut words)?.samples());
        for (plane, run) in planes
            .iter_mut()
            .zip(samples.chunks_exact(samples.len() / chunks.planes()))
        {
            plane.extend_from_slice(run);
        }
    }
    let report = image.finish()?;
    FitsChecksumPolicy::RequireValid.judge(report, idx)?;
    Ok(planes)
}

/// What reading the extension at HDU `idx` of `reader` holds beside the frame, from its header
/// — see [`ExtensionChunks::read_bytes`].
fn extension_read_bytes(reader: &StreamReader<File>, idx: usize) -> Result<u64, FitsLoadError> {
    let metadata = reader.hdus()[idx].image_metadata()?;
    Ok(ExtensionChunks::of(metadata.shape, metadata.bitpix.elem_size(), CHUNK_BYTES).read_bytes())
}

/// `source`, the error of opening HDU `idx`'s image, as a refusal when the HDU holds no image.
fn not_an_image(source: FitsError, idx: usize) -> FitsLoadError {
    match source {
        FitsError::HduKindMismatch {
            expected: ExpectedHdu::Image,
            ..
        } => FitsLoadError::from(FitsRefusal::NotAnImage { hdu: idx }),
        source => FitsLoadError::from(source),
    }
}

#[cfg(test)]
mod tests {
    use common::{CancelToken, TempDir, fs};
    use fits_well::FitsWriter;
    use fits_well::compress::{Compression, CompressionConfig, Quantization};
    use fits_well::image::{Image, Scaling};
    use fits_well::io::{BLOCK_SIZE, ChecksumReport, ChecksumStatus};

    use crate::internals::fits::fits_refusal;
    use crate::io::fits::decode::selected_fits::{
        self, extension_read_bytes, read_extension_in_chunks,
    };
    use crate::io::fits::error::FitsRefusal;
    use crate::io::load_ctx::LoadCtx;

    /// An extension is read a chunk of rows of every slice at a time straight into a plane per
    /// slice, and its checksum is summed across every chunk. A 3 × 5 × 3 cube of `f32`, 36 B a
    /// row of its three slices, read 72 B at a time: two rows, two rows, one, each split into the
    /// three slices, which come back as written, stored plain or compressed in one tile of every
    /// slice; a 3 × 5 byte plane read a row at a time is one plane. A flipped bit in the last
    /// slice is refused, as every extension requires a valid checksum.
    #[test]
    fn an_extension_reads_in_chunks_into_a_plane_per_slice() {
        let dir = TempDir::new("read_extension_chunks");
        let ctx = LoadCtx::new(CancelToken::never());
        let samples: Vec<f32> = (0..45).map(|idx| idx as f32).collect();
        let bytes: Vec<u8> = (0..15).collect();
        let path = dir.join("extension.fits");
        let mut writer = FitsWriter::new(fs::create(&path).unwrap()).with_checksums();
        writer
            .write_image(
                Image::new([3, 5, 3], samples.clone(), Scaling::IDENTITY)
                    .unwrap()
                    .view(),
                None,
            )
            .unwrap();
        writer
            .write_image(
                Image::new([3, 5], bytes.clone(), Scaling::IDENTITY)
                    .unwrap()
                    .view(),
                None,
            )
            .unwrap();
        #[expect(
            clippy::disallowed_methods,
            reason = "the test's own write, outside every loop"
        )]
        writer
            .write_compressed_image(
                Image::new([3, 5, 3], samples.clone(), Scaling::IDENTITY)
                    .unwrap()
                    .view(),
                &CompressionConfig::new(Compression::GZIP)
                    .with_tiles([3, 5, 3])
                    .with_quantization(Quantization::NONE),
                None,
            )
            .unwrap();
        drop(writer);

        let mut reader = selected_fits::open(&path, &ctx).unwrap();
        let planes = read_extension_in_chunks::<f32>(&mut reader, 0, 72).unwrap();
        assert_eq!(planes.len(), 3);
        for (slice, plane) in planes.iter().enumerate() {
            assert_eq!(plane, &samples[slice * 15..(slice + 1) * 15]);
        }
        let planes = read_extension_in_chunks::<u8>(&mut reader, 1, 1).unwrap();
        assert_eq!(planes.as_slice(), [bytes]);
        // The cube again, compressed as one tile of all three slices, which each chunk decodes.
        let planes = read_extension_in_chunks::<f32>(&mut reader, 2, 72).unwrap();
        for (slice, plane) in planes.iter().enumerate() {
            assert_eq!(plane, &samples[slice * 15..(slice + 1) * 15]);
        }
        // What a read in `CHUNK_BYTES` chunks holds, one chunk of every row: the chunk's words
        // rounded to 8 B, and the planes. The cube: its 180 B as 184, and 180 B of planes; the
        // byte plane: 15 B as 16, and 15 B.
        assert_eq!(extension_read_bytes(&reader, 0).unwrap(), 184 + 180);
        assert_eq!(extension_read_bytes(&reader, 1).unwrap(), 16 + 15);

        let mut corrupt = fs::read(&path).unwrap();
        // The last sample of the cube, 44.0, ends the first data block's 180 bytes.
        corrupt[BLOCK_SIZE + 179] ^= 0x01;
        fs::write(&path, corrupt).unwrap();
        let mut reader = selected_fits::open(&path, &ctx).unwrap();
        assert_eq!(
            fits_refusal(read_extension_in_chunks::<f32>(&mut reader, 0, 72).unwrap_err()),
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
