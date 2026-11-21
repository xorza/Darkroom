//! [`CfaImage`]: a sensor's single-channel mosaic before demosaicing, and what a peek at its
//! file says about it.

use std::f64::consts::FRAC_1_SQRT_2;
use std::path::Path;

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
use crate::image::cfa::mosaic_plane::MosaicPlane;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::frame::Frame;
use crate::image::linear_image::LinearImage;
use crate::image::masked_plane::MaskedPlane;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::metadata::image_provenance::DemosaicProvenance;
use crate::image::metadata::mosaic_noise::MosaicNoise;
use crate::io::error::WriteError;
use crate::io::fits;
use glam::USizeVec2;

/// Raw CFA image: one channel of sensor data under a color filter pattern, before demosaicing.
pub type CfaImage = Frame<MosaicPlane>;

/// [`CfaImage::data_mut_with_gain`]: the samples, and the flat gain beside them.
#[derive(Debug)]
pub(crate) struct MosaicEdit<'a> {
    pub(crate) data: &'a mut Buffer2<f32>,
    pub(crate) flat_gain: Option<&'a FrameGain>,
}

impl Frame<MosaicPlane> {
    /// Write the image, a sensor-domain image, to `path` as a checksummed floating-point FITS
    /// file, with its flags in a `LUMFLAGS` extension when they hold more than the NaN of a sample
    /// with no data; [`LoadCtx::cfa`](crate::LoadCtx::cfa) reads it back.
    pub fn write_fits(&self, path: impl AsRef<Path>) -> Result<(), WriteError> {
        fits::cfa::save(path.as_ref(), self)
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
        Self::of(MosaicPlane { data, cfa_type }, metadata)
    }

    /// The samples, row-major.
    pub const fn data(&self) -> &Buffer2<f32> {
        &self.samples.data
    }

    /// The samples to edit beside the flat gain that divided them: a split borrow, so an edit
    /// that reads the gain copies nothing of it.
    pub(crate) const fn data_mut_with_gain(&mut self) -> MosaicEdit<'_> {
        MosaicEdit {
            data: &mut self.samples.data,
            flat_gain: self.flat_gain.as_ref(),
        }
    }

    /// The samples, mutably: calibration's arithmetic on the mosaic.
    pub(crate) const fn data_mut(&mut self) -> &mut Buffer2<f32> {
        &mut self.samples.data
    }

    /// The sensor pattern the samples lie under.
    pub const fn cfa_type(&self) -> CfaType {
        self.samples.cfa_type
    }

    /// Replace every null with the median of its same-color neighbours.
    ///
    /// The demosaic reads a neighbourhood, so whatever sits under a null reaches output pixels the
    /// mask does not cover. The resampler normalizes by the weight of the taps that hold data
    /// (`resample::flagged_sources`), but an adaptive kernel like RCD or Markesteijn has no such
    /// weights. The decoder's frame-median fill would spread a value with no local meaning; a
    /// same-color neighbour median spreads a plausible one, so what escapes the mask is
    /// interpolation error rather than fabrication.
    ///
    /// The same repair a `DefectMap` applies to hot and cold pixels, through the same neighbour
    /// search — mask included, so a null is never repaired from another null, nor from a value
    /// another repair made. Each repaired null is flagged
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
    /// [`QualityFlags::REPAIRED`]; a null with no such neighbour stays a null. The mask keeps every
    /// null out of every repair, so the order of the repairs is free.
    fn repair_nulls_at(&mut self, nulls: &BitBuffer2, unmeasured: &BitBuffer2) {
        let lattice = CfaLattice::new(&self.samples.cfa_type);
        let size = self.size();
        let mut scratch = Gathered::default();
        let mut repaired = Vec::new();
        nulls.for_each_set(|pos| {
            let plane = MaskedPlane {
                pixels: &self.samples.data,
                mask: Some(unmeasured),
            };
            if let Some(value) = lattice.median(plane, pos, &mut scratch) {
                let idx = size.idx_of(pos);
                self.samples.data[idx] = value;
                repaired.push(idx);
            }
        });
        self.add_flags_at(QualityFlags::REPAIRED, &repaired);
    }

    /// Demosaic this CFA image into a 3-channel `LinearImage`, an X-Trans one with `passes`.
    /// `mosaic_noise` is the noise measured on this mosaic before: `None` exactly for a mono
    /// sensor.
    pub(crate) fn demosaic(
        mut self,
        passes: MarkesteijnPasses,
        mosaic_noise: Option<MosaicNoise>,
        cancel: &CancelToken,
    ) -> Result<LinearImage, Cancelled> {
        debug_assert_eq!(
            mosaic_noise.is_some(),
            self.samples.cfa_type.mosaic().is_some()
        );
        // Calibration repairs a light's nulls, and this leaves those alone; it repairs the rest: a
        // frame no calibration touched, or one read back from a file, which holds the decoder's
        // fill under each null.
        self.repair_nulls();
        let cfa_type = self.samples.cfa_type;
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
        // spreads as far as the demosaic reads. So does a photosite no repair could fill, every
        // neighbour of its color masked around it: its value measures nothing, and every pixel
        // the demosaic reads it into is `NO_DATA`.
        let mut flags = self.flags;
        if let Some(flags) = &mut flags {
            let unrepaired = |byte: u8| {
                let pixel = QualityFlags::from_byte(byte);
                pixel.intersects(QualityFlags::RESAMPLE_EXCLUDED)
                    && !pixel.intersects(QualityFlags::REPAIRED)
            };
            let spread = flags.bytes().iter().any(|&byte| unrepaired(byte)).then(|| {
                let mut spread = PixelFlags::from_fn(flags.size(), |idx| {
                    if unrepaired(flags.bytes()[idx]) {
                        QualityFlags::NO_DATA
                    } else {
                        QualityFlags::default()
                    }
                })
                .expect("a photosite is unrepaired");
                spread.dilate(demosaic.support(), QualityFlags::default());
                spread
            });
            flags.dilate(demosaic.support(), QualityFlags::RESAMPLE_EXCLUDED);
            if let Some(spread) = spread {
                flags.or_where(QualityFlags::NO_DATA, |idx| spread.is_flagged(idx));
            }
        }

        let mut image = LinearImage::new(
            demosaic.run(self.samples.data, gains, cancel)?,
            flags,
            metadata,
        );
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
    /// flags stay on it as `Self::demosaic` keeps them. One with no green neighbour, which only a frame narrower than its pattern has, keeps its own sample and
    /// flags. A mono frame is its own proxy, its metadata whole: nothing was filled, so its
    /// quantization step and flat gain still hold.
    pub fn green_proxy(&self) -> LinearImage {
        const GREEN: usize = CfaColor::Green.idx();
        let size = self.size();
        let desc = PlanarDesc::new(size, ChannelLayout::Mono);
        if self.samples.cfa_type == CfaType::Mono {
            let mut image =
                LinearImage::from_interleaved(desc, self.samples.data.pixels().to_vec());
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
        let cfa_type = self.samples.cfa_type;
        let flags = self.flags.as_ref();
        let mut pixels = vec![0.0f32; size.pixel_count()];
        let mut proxy_flags = flags.map(|_| vec![0u8; size.pixel_count()]);
        let rows = par::chunks_mut(&mut pixels, size.width).enumerate();
        let fill_row = |y: usize, row: &mut [f32], mut row_flags: Option<&mut [u8]>| {
            for (x, sample) in row.iter_mut().enumerate() {
                let idx = y * size.width + x;
                let own_flags = flags.map_or(0, |flags| flags.at(idx).byte());
                if cfa_type.slot_at(USizeVec2::new(x, y)) == GREEN {
                    *sample = self.samples.data[idx];
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
                            || cfa_type.slot_at(USizeVec2::new(nx, ny)) != GREEN
                        {
                            continue;
                        }
                        let tap = if dx == 0 || dy == 0 {
                            1.0
                        } else {
                            FRAC_1_SQRT_2
                        };
                        let neighbour = ny * size.width + nx;
                        sum += tap * f64::from(self.samples.data[neighbour]);
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
                    (self.samples.data[idx], own_flags)
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
