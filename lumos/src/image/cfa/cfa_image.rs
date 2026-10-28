//! [`CfaImage`]: a sensor's single-channel mosaic before demosaicing, and what a peek at its
//! file says about it.

use std::f64::consts::FRAC_1_SQRT_2;
use std::path::Path;

use arrayvec::ArrayVec;
use common::{Buffer2, CancelToken, par};
use imaginarium::{ChannelLayout, PlanarDesc};
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::error::Cancelled;
use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::cfa_lattice::{CfaLattice, Gathered};
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::Demosaic;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::frame_desc;
use crate::image::linear_image::LinearImage;
use crate::image::masked_plane::MaskedPlane;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::metadata::image_provenance::DemosaicProvenance;
use crate::image::metadata::mosaic_noise::MosaicNoise;
use crate::image::stackable_image::{ImageParts, StackableImage};
use crate::io::error::WriteError;
use crate::io::fits;
use common::{Size2us, Vec2us};

/// Raw CFA image - single channel with color filter pattern metadata.
/// Represents sensor data before demosaicing.
#[derive(Debug, Clone)]
pub struct CfaImage {
    /// Single-channel linear samples; calibration may put values outside `[0, 1]`.
    /// Layout: row-major, width * height pixels.
    data: Buffer2<f32>,
    cfa_type: CfaType,
    pub metadata: ImageMetadata,
    /// The data-quality flags of the pixels that carry any — see [`PixelFlags`]. The samples under
    /// [`QualityFlags::NO_DATA`] are a finite fill, not data.
    flags: Option<PixelFlags>,
    /// The gain a flat applied to each pixel's value and noise, by color — see [`FrameGain`]:
    /// set when calibration divides by a flat, and kept through a demosaic and through a FITS file
    /// of the mosaic. `None` for a frame no flat divided, and for a master, which the combine made.
    flat_gain: Option<FrameGain>,
}

impl StackableImage for CfaImage {
    fn desc(&self) -> PlanarDesc {
        PlanarDesc::new(self.size(), ChannelLayout::Mono)
    }

    fn flags(&self) -> Option<&PixelFlags> {
        self.flags.as_ref()
    }

    fn channel(&self, c: usize) -> &Buffer2<f32> {
        assert!(c == 0, "CfaImage has only 1 channel, got {c}");
        &self.data
    }

    fn metadata(&self) -> &ImageMetadata {
        &self.metadata
    }

    fn cfa_type(&self) -> Option<CfaType> {
        Some(self.cfa_type)
    }

    fn into_parts(self) -> ImageParts {
        let mut planes = ArrayVec::new();
        planes.push(self.data);
        ImageParts {
            planes,
            flags: self.flags,
            flat_gain: self.flat_gain,
        }
    }

    fn from_parts(parts: ImageParts, cfa_type: Option<CfaType>, metadata: ImageMetadata) -> Self {
        let ImageParts {
            planes,
            flags,
            flat_gain,
        } = parts;
        assert_eq!(planes.len(), 1, "a mosaic is one plane");
        let data = planes.into_iter().next().expect("one plane");
        let mut image = Self::from_plane(
            data,
            cfa_type.expect("a mosaic states its pattern"),
            metadata,
        );
        image.set_flags(flags);
        image.set_flat_gain(flat_gain);
        image
    }

    fn flat_gain(&self) -> Option<&FrameGain> {
        Self::flat_gain(self)
    }
}

impl CfaImage {
    /// Write the image, a sensor-domain image, to `path` as a checksummed floating-point FITS
    /// file, with its flags in a `LUMFLAGS` extension when they hold more than the NaN of a sample
    /// with no data; [`LoadCtx::cfa`](crate::LoadCtx::cfa) reads it back.
    pub fn write_fits(&self, path: impl AsRef<Path>) -> Result<(), WriteError> {
        fits::cfa::save(path.as_ref(), self)
    }

    /// The sensor extent the samples cover.
    pub(crate) const fn size(&self) -> Size2us {
        self.data.size()
    }

    /// OR `flags` into every pixel whose row-major index satisfies `holds`, creating the flag
    /// plane when the image has none and some pixel does.
    pub(crate) fn add_flags_where(
        &mut self,
        flags: QualityFlags,
        holds: impl Fn(usize) -> bool + Sync,
    ) {
        let size = self.size();
        match &mut self.flags {
            Some(existing) => {
                debug_assert_eq!(existing.size(), size, "flags for another geometry");
                existing.or_where(flags, holds);
            }
            None => {
                self.flags = PixelFlags::from_fn(size, |idx| {
                    if holds(idx) {
                        flags
                    } else {
                        QualityFlags::default()
                    }
                });
            }
        }
    }

    /// OR `flags` into the pixels at `idxs`, creating the flag plane when the image has none
    /// and some index is given.
    pub(crate) fn add_flags_at(&mut self, flags: QualityFlags, idxs: &[usize]) {
        let size = self.size();
        match &mut self.flags {
            Some(existing) => {
                debug_assert_eq!(existing.size(), size, "flags for another geometry");
                existing.or_at(flags, idxs);
            }
            None => self.flags = PixelFlags::at_idxs(size, flags, idxs),
        }
    }

    /// Create an in-memory sensor image whose CFA classification is supplied by the caller.
    ///
    /// # Panics
    /// When `data` holds no pixel, or has a side past [`MAX_SIDE`](crate::frame_desc::MAX_SIDE).
    pub fn from_plane(data: Buffer2<f32>, cfa_type: CfaType, metadata: ImageMetadata) -> Self {
        frame_desc::assert_frame(PlanarDesc::new(data.size(), ChannelLayout::Mono));
        Self {
            data,
            cfa_type,
            metadata,
            flags: None,
            flat_gain: None,
        }
    }

    /// The samples, row-major.
    pub const fn data(&self) -> &Buffer2<f32> {
        &self.data
    }

    /// The samples, mutably: calibration's arithmetic on the mosaic.
    pub(crate) const fn data_mut(&mut self) -> &mut Buffer2<f32> {
        &mut self.data
    }

    /// The sensor pattern the samples lie under.
    pub const fn cfa_type(&self) -> CfaType {
        self.cfa_type
    }

    /// Resident RAM held by this frame: its f32 CFA plane, its flag plane and a flat gain of its
    /// own. A shared gain is counted by its owner, the run's shared gains. Metadata is negligible
    /// against a full-sensor plane.
    pub fn ram_bytes(&self) -> usize {
        self.data.width() * self.data.height() * size_of::<f32>()
            + self.flags.as_ref().map_or(0, |flags| flags.bytes().len())
            + self.flat_gain.as_ref().map_or(0, FrameGain::own_ram_bytes)
    }

    /// The data-quality flags of the pixels; `None` when no pixel carries one.
    pub const fn flags(&self) -> Option<&PixelFlags> {
        self.flags.as_ref()
    }

    /// Replace the flags with `flags`, of these pixels.
    pub(crate) fn set_flags(&mut self, flags: Option<PixelFlags>) {
        debug_assert!(
            flags
                .as_ref()
                .is_none_or(|flags| flags.size() == self.size()),
            "flags for another geometry"
        );
        self.flags = flags;
    }

    /// The flags, moved out.
    pub(crate) const fn take_flags(&mut self) -> Option<PixelFlags> {
        self.flags.take()
    }

    /// The gain a flat applied to the pixels, by color, for a mosaic a flat divided.
    pub const fn flat_gain(&self) -> Option<&FrameGain> {
        self.flat_gain.as_ref()
    }

    /// Replace the flat gain with `gain`, which covers these pixels and the pattern's colors.
    pub(crate) fn set_flat_gain(&mut self, gain: Option<FrameGain>) {
        debug_assert!(
            gain.as_ref()
                .is_none_or(|gain| gain.covers(self.size(), self.cfa_type.color_count())),
            "an image's flat gain covers another image's pixels"
        );
        self.flat_gain = gain;
    }

    /// Replace every null with the median of its same-color neighbours.
    ///
    /// The demosaic reads a neighbourhood, so whatever sits under a null reaches output pixels the
    /// mask does not cover. The resampler answers the same problem by sampling over the taps that
    /// hold data and normalizing by their weight — `resample::flagged_sources` — but an adaptive
    /// kernel like RCD or Markesteijn offers no such weights to normalize by. Leaving the decoder's
    /// frame-median fill there would spread a value with no local meaning; a same-color neighbour
    /// median spreads a plausible one, so what escapes the mask is interpolation error rather than
    /// fabrication.
    ///
    /// The same repair a `DefectMap` applies to hot and cold pixels, for the same
    /// reason and through the same neighbour search — mask included, so a null is never repaired
    /// from another null, nor from a value another repair made. Each repaired null is flagged
    /// [`QualityFlags::REPAIRED`] beside its `NO_DATA`, and a null already flagged so is left as
    /// it is. Calibration owns the repair and runs it once its arithmetic is done, so a pixel a
    /// master left without a measurement holds a fill, not a difference against a bound.
    pub(crate) fn repair_nulls(&mut self) {
        let Some(nulls) = self.unrepaired_nulls() else {
            return;
        };
        let unmeasured = self.unmeasured_mask().expect("a null is flagged");
        self.repair_nulls_at(&nulls, &unmeasured);
    }

    /// [`Self::repair_nulls`] where `unmeasured` already covers every pixel no repair may read:
    /// the [`Self::unmeasured_mask`] calibration built, and grew by the defects it repaired.
    pub(crate) fn repair_nulls_within(&mut self, unmeasured: &BitBuffer2) {
        if let Some(nulls) = self.unrepaired_nulls() {
            self.repair_nulls_at(&nulls, unmeasured);
        }
    }

    /// The pixels no repair may read: every null and every pixel a repair made; `None` for a frame
    /// that holds neither.
    pub(crate) fn unmeasured_mask(&self) -> Option<BitBuffer2> {
        let unmeasured = QualityFlags::NO_DATA.union(QualityFlags::REPAIRED);
        self.flags()
            .filter(|flags| flags.contains(unmeasured))
            .map(|flags| flags.mask_of(unmeasured))
    }

    /// The nulls no repair has filled yet; `None` for a frame that holds none.
    fn unrepaired_nulls(&self) -> Option<BitBuffer2> {
        let bytes = self
            .flags()
            .filter(|flags| flags.contains(QualityFlags::NO_DATA))?
            .bytes();
        let mut nulls = BitBuffer2::new_default(self.size());
        nulls.fill_from_predicate(|idx| {
            let flags = QualityFlags::from_byte(bytes[idx]);
            flags.intersects(QualityFlags::NO_DATA) && !flags.intersects(QualityFlags::REPAIRED)
        });
        (nulls.count_ones() > 0).then_some(nulls)
    }

    /// Fill each of `nulls` from its same-color neighbours outside `unmeasured`, and flag it
    /// [`QualityFlags::REPAIRED`]. The mask keeps every null out of every repair, so the order of
    /// the repairs is free.
    fn repair_nulls_at(&mut self, nulls: &BitBuffer2, unmeasured: &BitBuffer2) {
        let lattice = CfaLattice::new(&self.cfa_type);
        let size = self.size();
        let mut scratch = Gathered::default();
        nulls.for_each_set(|pos| {
            let plane = MaskedPlane {
                pixels: &self.data,
                mask: Some(unmeasured),
            };
            let repaired = lattice.median(plane, pos, &mut scratch);
            self.data[size.idx_of(pos)] = repaired;
        });
        self.add_flags_where(QualityFlags::REPAIRED, |idx| nulls.get(idx));
    }

    /// Demosaic this CFA image into a 3-channel `LinearImage`, an X-Trans one with `passes`, its
    /// `mosaic_noise` measured on this mosaic before: `None` exactly for a mono sensor. Consumes
    /// self.
    pub(crate) fn demosaic(
        mut self,
        passes: MarkesteijnPasses,
        mosaic_noise: Option<MosaicNoise>,
        cancel: &CancelToken,
    ) -> Result<LinearImage, Cancelled> {
        debug_assert_eq!(mosaic_noise.is_some(), self.cfa_type.mosaic().is_some());
        // Calibration repairs a light's nulls, and this leaves those alone; it repairs the rest: a
        // frame no calibration touched, or one read back from a file, which holds the decoder's
        // fill under each null.
        self.repair_nulls();
        let cfa_type = self.cfa_type;
        let demosaic = Demosaic::of(cfa_type, passes);
        // Interpolation correlates neighbouring samples, which hides part of their noise from any
        // later measurement, and mixes them, so one step's σ no longer bounds any of them. A mono
        // sensor's samples pass through untouched and keep it.
        let mut metadata = self.metadata;
        if let Some(provenance) = metadata.derived.provenance_mut() {
            provenance.color = cfa_type.planes_color();
            provenance.demosaic = demosaic.provenance();
        }
        if cfa_type.mosaic().is_some() {
            metadata.derived.after_interpolation(mosaic_noise);
        }
        // The direction decisions compare neighbours of different colors, so the kernels balance
        // the colors by these gains as they read and take them back out as they write (see
        // `tiled`). Relative to green, so the samples keep the sensor's balance.
        let gains = (cfa_type != CfaType::Mono)
            .then_some(metadata.header.camera_white_balance)
            .flatten()
            .map_or([1.0; 3], |[red, green, blue, _]| {
                [red / green, 1.0, blue / green]
            });
        // A photosite whose value measured nothing — a null, a defect, a hit — holds its
        // same-color neighbours' value by now, from `repair_nulls` above, the defect correction
        // and the cosmic-ray in-painting, so the demosaic reads an estimate there as it reads one
        // for every color a photosite lacks. Those flags stay at their own extent, as Siril,
        // PixInsight's CosmeticCorrection and dcraw's `.badpixels` repair before the demosaic and
        // take the result as data; grown by the demosaic's reach, a few percent of warm pixels
        // would flag every pixel. A bound, saturated or at the flat's floor, is not repaired, and
        // spreads as far as the demosaic reads.
        let mut flags = self.flags;
        if let Some(flags) = &mut flags {
            debug_assert!(
                flags.bytes().iter().all(|&byte| {
                    let pixel = QualityFlags::from_byte(byte);
                    !pixel.intersects(QualityFlags::RESAMPLE_EXCLUDED)
                        || pixel.intersects(QualityFlags::REPAIRED)
                }),
                "a photosite that measured nothing reached the demosaic unrepaired"
            );
            flags.dilate(demosaic.support(), QualityFlags::RESAMPLE_EXCLUDED);
        }

        let mut image = LinearImage::new(demosaic.run(self.data, gains, cancel)?, flags, metadata);
        image.set_flat_gain(self.flat_gain);
        Ok(image)
    }

    /// A plane to register this mosaic on, as Siril registers CFA frames for its CFA drizzle
    /// (`interpolate_nongreen`): each green photosite as it is, and every other filled from the
    /// green photosites of its 3×3 neighbourhood, weighed 1 beside it and 1/√2 on a diagonal. On
    /// a Bayer mosaic that is the mean of its four green neighbours, on X-Trans of the hexagon's.
    /// Stars are measured on green alone, at its full resolution, with no color interpolated
    /// across an edge as a demosaic would.
    ///
    /// A filled photosite carries the flags of the greens it was filled from, but a repaired
    /// green's own: its value is an estimate the fill reads as the demosaic reads it, and its
    /// flags stay on it as [`Self::demosaic`] keeps them. One with no green neighbour, which only a frame narrower than its pattern has, keeps its own sample and
    /// flags. A mono frame is its own proxy, its metadata whole: nothing was filled, so its
    /// quantization step and flat gain still hold.
    pub fn green_proxy(&self) -> LinearImage {
        const GREEN: usize = CfaColor::Green.idx();
        let size = self.size();
        let desc = PlanarDesc::new(size, ChannelLayout::Mono);
        if self.cfa_type == CfaType::Mono {
            let mut image = LinearImage::from_interleaved(desc, self.data.pixels().to_vec());
            image.metadata = self.metadata.clone();
            image.set_flags(self.flags.clone());
            image.set_flat_gain(self.flat_gain.clone());
            return image;
        }
        let mut metadata = self.metadata.clone();
        metadata.derived.after_interpolation(None);
        if let Some(provenance) = metadata.derived.provenance_mut() {
            provenance.demosaic = DemosaicProvenance::GreenProxy;
        }
        let cfa_type = self.cfa_type;
        let flags = self.flags.as_ref();
        let mut pixels = vec![0.0f32; size.pixel_count()];
        let mut proxy_flags = flags.map(|_| vec![0u8; size.pixel_count()]);
        let rows = par::chunks_mut(&mut pixels, size.width).enumerate();
        let fill_row = |y: usize, row: &mut [f32], mut row_flags: Option<&mut [u8]>| {
            for (x, sample) in row.iter_mut().enumerate() {
                let idx = y * size.width + x;
                let own_flags = flags.map_or(0, |flags| flags.at(idx).byte());
                if cfa_type.slot_at(Vec2us::new(x, y)) == GREEN {
                    *sample = self.data[idx];
                    if let Some(row_flags) = row_flags.as_deref_mut() {
                        row_flags[x] = own_flags;
                    }
                    continue;
                }
                let (mut sum, mut weight, mut carried) = (0.0f64, 0.0f64, 0u8);
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        let (Some(nx), Some(ny)) =
                            (x.checked_add_signed(dx), y.checked_add_signed(dy))
                        else {
                            continue;
                        };
                        if (dx, dy) == (0, 0)
                            || nx >= size.width
                            || ny >= size.height
                            || cfa_type.slot_at(Vec2us::new(nx, ny)) != GREEN
                        {
                            continue;
                        }
                        let tap = if dx == 0 || dy == 0 {
                            1.0
                        } else {
                            FRAC_1_SQRT_2
                        };
                        let neighbour = ny * size.width + nx;
                        sum += tap * f64::from(self.data[neighbour]);
                        weight += tap;
                        let green =
                            flags.map_or_else(QualityFlags::default, |flags| flags.at(neighbour));
                        let passed = if green.intersects(QualityFlags::REPAIRED) {
                            green.intersection(QualityFlags::RESAMPLE_CARRIED)
                        } else {
                            green
                        };
                        carried |= passed.byte();
                    }
                }
                let (value, value_flags) = if weight > 0.0 {
                    ((sum / weight) as f32, carried)
                } else {
                    (self.data[idx], own_flags)
                };
                *sample = value;
                if let Some(row_flags) = row_flags.as_deref_mut() {
                    row_flags[x] = value_flags;
                }
            }
        };
        match proxy_flags.as_mut() {
            Some(proxy_flags) => rows
                .zip(par::chunks_mut(proxy_flags, size.width))
                .for_each(|((y, row), row_flags)| fill_row(y, row, Some(row_flags))),
            None => rows.for_each(|(y, row)| fill_row(y, row, None)),
        }
        let mut image = LinearImage::from_interleaved(desc, pixels);
        image.metadata = metadata;
        image.set_flags(
            proxy_flags.and_then(|bytes| PixelFlags::from_bytes(Buffer2::new(size, bytes))),
        );
        image
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use common::CancelToken;

    use crate::image::cfa::cfa_image::CfaImage;
    use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
    use crate::image::linear_image::LinearImage;
    use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;

    impl CfaImage {
        /// The mosaic demosaiced as a decoder demosaics it, an X-Trans one in `passes`: its noise
        /// measured first, with no cancel.
        pub(crate) fn demosaic_measured(self, passes: MarkesteijnPasses) -> LinearImage {
            let noise = MosaicNoiseMeasure::of(&self).map(MosaicNoiseMeasure::measure);
            self.demosaic(passes, noise, &CancelToken::never())
                .expect("a token that never cancels")
        }
    }
}
