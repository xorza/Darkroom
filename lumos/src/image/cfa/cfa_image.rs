//! [`CfaImage`]: a sensor's single-channel mosaic before demosaicing, and what a peek at its
//! file says about it.

use std::f64::consts::FRAC_1_SQRT_2;

use arrayvec::ArrayVec;
use common::CancelToken;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::error::Cancelled;
use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::cfa_lattice::{CfaLattice, Gathered};
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::Demosaic;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::channel_layout::ChannelLayout;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::image_provenance::DemosaicProvenance;
use crate::image::linear_image::LinearImage;
use crate::image::masked_plane::MaskedPlane;
use crate::image::mosaic_noise::MosaicNoise;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::image::stackable_image::{ImageParts, StackableImage};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

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
    fn dimensions(&self) -> ImageDimensions {
        ImageDimensions::new(self.size(), ChannelLayout::Mono)
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
    /// The sensor extent the samples cover.
    pub(crate) const fn size(&self) -> Size2us {
        Size2us::new(self.data.width(), self.data.height())
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
    pub const fn from_plane(
        data: Buffer2<f32>,
        cfa_type: CfaType,
        metadata: ImageMetadata,
    ) -> Self {
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

    /// Resident RAM held by this frame: its f32 CFA plane and its flag plane. A flat gain is
    /// shared with every frame the flat divided, so its owner counts it. Metadata is negligible
    /// against a full-sensor plane.
    pub fn ram_bytes(&self) -> usize {
        self.data.width() * self.data.height() * size_of::<f32>()
            + self.flags.as_ref().map_or(0, |flags| flags.bytes().len())
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
        let Some(flags) = self
            .flags()
            .filter(|flags| flags.contains(QualityFlags::NO_DATA))
        else {
            return;
        };
        let lattice = CfaLattice::new(&self.cfa_type);
        let size = self.size();
        let mut nulls = flags.mask_of(QualityFlags::NO_DATA);
        nulls.and_not(&flags.mask_of(QualityFlags::REPAIRED));
        let mask = flags.mask_of(QualityFlags::NO_DATA.union(QualityFlags::REPAIRED));
        let mut scratch = Gathered::default();
        // The mask keeps every null out of every repair, so the order of the repairs is free.
        nulls.for_each_set(|pos| {
            let plane = MaskedPlane {
                pixels: &self.data,
                mask: Some(&mask),
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
        // `NO_DATA` travels at its own extent, which `repair_nulls` above is what makes honest:
        // these pixels were reconstructed rather than measured, and the combine still has to know
        // that. Every other fact spreads as far as the demosaic reads.
        let mut flags = self.flags;
        if let Some(flags) = &mut flags {
            flags.dilate(demosaic.support(), QualityFlags::NO_DATA);
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
    /// A filled photosite carries the flags of the greens it was filled from. One with no green
    /// neighbour, which only a frame narrower than its pattern has, keeps its own sample and
    /// flags. A mono frame is its own proxy, its metadata whole: nothing was filled, so its
    /// quantization step and flat gain still hold.
    pub fn green_proxy(&self) -> LinearImage {
        const GREEN: usize = CfaColor::Green.idx();
        let size = self.size();
        let dimensions = ImageDimensions::new(size, ChannelLayout::Mono);
        if self.cfa_type == CfaType::Mono {
            let mut image = LinearImage::from_interleaved(dimensions, self.data.pixels().to_vec());
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
        let rows = pixels.par_chunks_mut(size.width).enumerate();
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
                        carried |= flags.map_or(0, |flags| flags.at(neighbour).byte());
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
                .zip(proxy_flags.par_chunks_mut(size.width))
                .for_each(|((y, row), row_flags)| fill_row(y, row, Some(row_flags))),
            None => rows.for_each(|(y, row)| fill_row(y, row, None)),
        }
        let mut image = LinearImage::from_interleaved(dimensions, pixels);
        image.metadata = metadata;
        image.set_flags(proxy_flags.and_then(|bytes| {
            PixelFlags::from_bytes(Buffer2::new(size.width, size.height, bytes))
        }));
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
