//! [`PlaneHdu`]: an image HDU written from the planes the caller holds, a chunk of rows at a time.

use std::io::{Seek, Write};
use std::mem;
use std::ops::Range;

use arrayvec::ArrayVec;
use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::{Bitpix, Data, Scaling};
use rayon::prelude::*;

use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::io::error::WriteError;
use crate::io::fits::CHUNK_BYTES;

/// A sample type an HDU stores, and the owned [`Data`] a chunk of it is written as.
pub(crate) trait PlaneSample: Copy + Send + Sync {
    const BITPIX: Bitpix;

    fn into_data(samples: Vec<Self>) -> Data;

    /// The samples of `data`, which [`Self::into_data`] made.
    fn from_data(data: Data) -> Vec<Self>;
}

impl PlaneSample for f32 {
    const BITPIX: Bitpix = Bitpix::F32;

    fn into_data(samples: Vec<Self>) -> Data {
        Data::F32(samples)
    }

    fn from_data(data: Data) -> Vec<Self> {
        let Data::F32(samples) = data else {
            unreachable!("a chunk of f32 samples is F32 data");
        };
        samples
    }
}

impl PlaneSample for u8 {
    const BITPIX: Bitpix = Bitpix::U8;

    fn into_data(samples: Vec<Self>) -> Data {
        Data::U8(samples)
    }

    fn from_data(data: Data) -> Vec<Self> {
        let Data::U8(samples) = data else {
            unreachable!("a chunk of bytes is U8 data");
        };
        samples
    }
}

/// What an HDU's samples are made of, read a range of a slice's pixels at a time.
pub(crate) trait PlaneSource: Sync {
    type Sample: PlaneSample;

    /// Append the samples of `pixels`, pixel indices of slice `slice`, to `out`.
    fn extend(&self, slice: usize, pixels: Range<usize>, out: &mut Vec<Self::Sample>);
}

/// `f32` planes, a slice each, as they are but at each pixel `nulls` flags as holding no data:
/// NaN there, the standard's own blank, so a reader that does not know `LUMFLAGS` still sees no
/// measurement.
#[derive(Debug)]
pub(crate) struct FloatPlanes<'a> {
    planes: ArrayVec<&'a [f32], 3>,
    nulls: Option<&'a PixelFlags>,
}

impl<'a> FloatPlanes<'a> {
    /// `planes` as they are.
    pub(crate) const fn new(planes: ArrayVec<&'a [f32], 3>) -> Self {
        Self {
            planes,
            nulls: None,
        }
    }

    /// `planes`, blank at each pixel `flags` holds no data at.
    pub(crate) fn blanking(planes: ArrayVec<&'a [f32], 3>, flags: Option<&'a PixelFlags>) -> Self {
        Self {
            planes,
            nulls: flags.filter(|flags| flags.contains(QualityFlags::NO_DATA)),
        }
    }
}

impl PlaneSource for FloatPlanes<'_> {
    type Sample = f32;

    fn extend(&self, slice: usize, pixels: Range<usize>, out: &mut Vec<f32>) {
        let samples = &self.planes[slice][pixels.clone()];
        match self.nulls {
            None => out.extend_from_slice(samples),
            Some(nulls) => out.par_extend(samples.par_iter().zip(&nulls.bytes()[pixels]).map(
                |(&sample, &byte)| {
                    if QualityFlags::from_byte(byte).intersects(QualityFlags::NO_DATA) {
                        f32::NAN
                    } else {
                        sample
                    }
                },
            )),
        }
    }
}

/// The bytes of a `LUMFLAGS` extension: each pixel's flags, but [`QualityFlags::REPAIRED`] at a
/// pixel of no data, where the file holds NaN, not the repair, and a reader fills it again.
#[derive(Debug)]
pub(crate) struct FlagBytes<'a>(pub(crate) &'a PixelFlags);

impl FlagBytes<'_> {
    /// The byte the extension stores for a pixel of flags `byte`.
    pub(crate) const fn stored(byte: u8) -> u8 {
        if QualityFlags::from_byte(byte).intersects(QualityFlags::NO_DATA) {
            byte & !QualityFlags::REPAIRED.byte()
        } else {
            byte
        }
    }
}

impl PlaneSource for FlagBytes<'_> {
    type Sample = u8;

    fn extend(&self, _: usize, pixels: Range<usize>, out: &mut Vec<u8>) {
        out.par_extend(
            self.0.bytes()[pixels]
                .par_iter()
                .map(|&byte| Self::stored(byte)),
        );
    }
}

/// The mask astropy's `CCDData` reads: 1 at each pixel of no data, 0 elsewhere, every slice
/// alike.
#[derive(Debug)]
pub(crate) struct NoDataMask<'a>(pub(crate) &'a PixelFlags);

impl PlaneSource for NoDataMask<'_> {
    type Sample = u8;

    fn extend(&self, _: usize, pixels: Range<usize>, out: &mut Vec<u8>) {
        out.par_extend(self.0.bytes()[pixels].par_iter().map(|&byte| {
            u8::from(QualityFlags::from_byte(byte).intersects(QualityFlags::NO_DATA))
        }));
    }
}

/// One image HDU written from the planes the caller holds: its header, its `shape` — width,
/// height, and the slices of a cube when it has a third axis — its scaling, and the `source` of
/// its samples, which the write reads a chunk of rows at a time into one buffer it reuses.
#[derive(Debug)]
pub(crate) struct PlaneHdu<S> {
    pub(crate) header: Header,
    pub(crate) shape: ArrayVec<usize, 3>,
    pub(crate) scaling: Scaling,
    pub(crate) source: S,
}

impl<S: PlaneSource> PlaneHdu<S> {
    /// Write the HDU to `writer`, the bytes [`FitsWriter::write_image`] writes of the same image,
    /// holding beside the planes a chunk of at most [`CHUNK_BYTES`] of samples and fits-well's
    /// encoding of it.
    pub(crate) fn write<W: Write + Seek>(
        &self,
        writer: &mut FitsWriter<W>,
    ) -> Result<(), WriteError> {
        self.write_in_chunks(writer, CHUNK_BYTES)
    }

    /// [`Self::write`] in chunks of at most `chunk_bytes` of samples, a row at least.
    fn write_in_chunks<W: Write + Seek>(
        &self,
        writer: &mut FitsWriter<W>,
        chunk_bytes: usize,
    ) -> Result<(), WriteError> {
        debug_assert!(self.shape.len() >= 2, "an image HDU is a plane at least");
        let (width, height) = (self.shape[0], self.shape[1]);
        let rows = (chunk_bytes / (width * size_of::<S::Sample>()).max(1)).clamp(1, height);
        let mut stream = writer.stream_image(
            &self.shape,
            S::Sample::BITPIX,
            self.scaling,
            Some(&self.header),
        )?;
        let mut chunk = Vec::with_capacity(rows * width);
        for slice in 0..self.shape.get(2).copied().unwrap_or(1) {
            for start in (0..height).step_by(rows) {
                chunk.clear();
                let end = (start + rows).min(height);
                self.source
                    .extend(slice, start * width..end * width, &mut chunk);
                let data = S::Sample::into_data(mem::take(&mut chunk));
                stream.write_chunk(&data)?;
                chunk = S::Sample::from_data(data);
            }
        }
        stream.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use arrayvec::ArrayVec;
    use fits_well::FitsWriter;
    use fits_well::header::Header;
    use fits_well::image::{Image, Scaling};

    use crate::image::pixel_flags::PixelFlags;
    use crate::image::quality_flags::QualityFlags;
    use crate::io::fits::plane_hdu::{FlagBytes, FloatPlanes, NoDataMask, PlaneHdu, PlaneSource};
    use crate::math::size2us::Size2us;

    /// The file `hdu` writes in chunks of 8 bytes — a row of `f32` samples of a 3-wide plane at
    /// a time, two rows of bytes — which must be the file fits-well writes whole of `image`.
    fn streams_as_written<S: PlaneSource>(hdu: &PlaneHdu<S>, image: &Image) {
        let mut streamed = FitsWriter::new(Cursor::new(Vec::new())).with_checksums();
        hdu.write_in_chunks(&mut streamed, 8).unwrap();
        let mut whole = FitsWriter::new(Cursor::new(Vec::new())).with_checksums();
        whole.write_image(image, Some(&hdu.header)).unwrap();
        assert_eq!(
            streamed.into_inner().into_inner(),
            whole.into_inner().into_inner()
        );
    }

    /// Each source streams the bytes fits-well writes whole of the samples it stands for, a row
    /// of a 3 × 2 image at a time, over every slice. Pixel 4 holds no data and was repaired, and
    /// pixel 1 saturated. Two scaled planes as a cube are NaN at pixel 4; the flag bytes keep the
    /// saturation and drop the repair at the null; the mask is 1 at pixel 4 in both slices.
    #[test]
    fn every_source_streams_the_image_fits_well_writes_whole() {
        let size = Size2us::new(3, 2);
        let null = QualityFlags::NO_DATA.union(QualityFlags::REPAIRED);
        let flags = PixelFlags::from_fn(size, |idx| match idx {
            1 => QualityFlags::SATURATED,
            4 => null,
            _ => QualityFlags::default(),
        })
        .unwrap();
        let shape = |slices: Option<usize>| -> ArrayVec<usize, 3> {
            [3, 2].into_iter().chain(slices).collect()
        };
        let mut header = Header::new();
        header.set("OBJECT", "streamed").unwrap();
        let red: Vec<f32> = (0..6).map(|idx| idx as f32 / 7.0).collect();
        let green: Vec<f32> = (0..6).map(|idx| (idx + 10) as f32 / 7.0).collect();
        let scaling = Scaling {
            bscale: 2.5,
            ..Scaling::IDENTITY
        };

        let mut blanked: Vec<f32> = red.iter().chain(&green).copied().collect();
        blanked[4] = f32::NAN;
        blanked[10] = f32::NAN;
        streams_as_written(
            &PlaneHdu {
                header: header.clone(),
                shape: shape(Some(2)),
                scaling,
                source: FloatPlanes::blanking(
                    [red.as_slice(), green.as_slice()].into_iter().collect(),
                    Some(&flags),
                ),
            },
            &Image::new_scaled(vec![3, 2, 2], blanked, scaling).unwrap(),
        );
        streams_as_written(
            &PlaneHdu {
                header: header.clone(),
                shape: shape(None),
                scaling: Scaling::IDENTITY,
                source: FloatPlanes::new([red.as_slice()].into_iter().collect()),
            },
            &Image::new(vec![3, 2], red.clone()).unwrap(),
        );
        streams_as_written(
            &PlaneHdu {
                header: header.clone(),
                shape: shape(None),
                scaling: Scaling::IDENTITY,
                source: FlagBytes(&flags),
            },
            &Image::new(
                vec![3, 2],
                vec![
                    0,
                    QualityFlags::SATURATED.byte(),
                    0,
                    0,
                    QualityFlags::NO_DATA.byte(),
                    0,
                ],
            )
            .unwrap(),
        );
        streams_as_written(
            &PlaneHdu {
                header,
                shape: shape(Some(2)),
                scaling: Scaling::IDENTITY,
                source: NoDataMask(&flags),
            },
            &Image::new(vec![3, 2, 2], vec![0u8, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0]).unwrap(),
        );
    }
}
