//! The scenes written as the files a run reads, each set in a directory of its own that goes with
//! it.

use std::path::{Path, PathBuf};

use common::TempDir;
use fits_well::header::Header;
use fits_well::image::Image;

use crate::bench::scene::{self, FRAME, Field};
use crate::calibration::calibration_set::CalibrationSet;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::linear_image::LinearImage;
use crate::internals::fits::write_fits;
use crate::io::writer;

/// The lights of the end-to-end sets: few enough that an iteration stays near ten seconds.
pub(crate) const LIGHTS: usize = 12;

/// A set of files and the directory that holds them, removed with it.
#[derive(Debug)]
pub(crate) struct Files {
    pub(crate) paths: Vec<PathBuf>,
    dir: TempDir,
}

impl Files {
    /// [`LIGHTS`] aligned exposures of `field`, calibrated, mono, as 32-bit float FITS: a set
    /// calibrated and registered elsewhere.
    pub(crate) fn aligned_lights(field: Field) -> Self {
        Self::write("aligned", |path, index| {
            write_mono_f32(path, &field.aligned(FRAME, index).image);
        })
    }

    /// [`LIGHTS`] dithered exposures of `field` as a color camera writes them: 16-bit mosaics
    /// behind `cfa_type`'s filters, the pattern in `BAYERPAT`.
    pub(crate) fn raw_lights(field: Field, cfa_type: CfaType) -> Self {
        Self::write("raw", |path, index| {
            write_mosaic_u16(path, &field.light_exposure(FRAME, cfa_type, index));
        })
    }

    /// [`LIGHTS`] dithered exposures of `field` as calibration leaves them, mosaics behind
    /// `cfa_type`'s filters saved by `writer::cfa_fits`: a set to drizzle.
    pub(crate) fn calibrated_mosaics(field: Field, cfa_type: CfaType) -> Self {
        Self::write("calibrated", |path, index| {
            writer::cfa_fits(&field.mosaic_exposure(FRAME, cfa_type, index), path).unwrap();
        })
    }

    fn write(name: &str, write: impl Fn(&Path, usize)) -> Self {
        let dir = TempDir::new("lumos-bench-files");
        let paths = (0..LIGHTS)
            .map(|index| {
                let path = dir.join(format!("{name}_{index:02}.fits"));
                write(&path, index);
                path
            })
            .collect();
        Self { paths, dir }
    }

    pub(crate) fn dir(&self) -> &Path {
        self.dir.path()
    }
}

/// [`LIGHTS`] frames of each of the bias, dark and flat sequences of the sensor behind `cfa_type`,
/// as [`Files::raw_lights`] writes lights.
#[derive(Debug)]
pub(crate) struct MasterFiles {
    pub(crate) frames: CalibrationSet<Vec<PathBuf>>,
    _dir: TempDir,
}

impl MasterFiles {
    pub(crate) fn of(cfa_type: CfaType) -> Self {
        let dir = TempDir::new("lumos-bench-masters");
        let mut frames = CalibrationSet::<Vec<PathBuf>>::default();
        for index in 0..LIGHTS {
            let set = scene::master_frames(FRAME, cfa_type, index);
            for (name, frame, paths) in [
                ("bias", set.bias, &mut frames.bias),
                ("dark", set.dark, &mut frames.dark),
                ("flat", set.flat, &mut frames.flat),
            ] {
                let path = dir.join(format!("{name}_{index:02}.fits"));
                write_mosaic_u16(&path, &frame.expect("every sequence has a frame"));
                paths.push(path);
            }
        }
        Self { frames, _dir: dir }
    }
}

/// `image`'s one plane as normalized 32-bit floats.
pub(crate) fn write_mono_f32(path: &Path, image: &LinearImage) {
    let plane = image.channel(0);
    let shape = vec![plane.width(), plane.height()];
    write_fits(
        path,
        &Image::new(shape, plane.pixels().to_vec()).unwrap(),
        None,
    );
}

/// `mosaic` as a camera's 16-bit file: `BITPIX = 16` with `BZERO = 32768`, and for a Bayer mosaic
/// its pattern in `BAYERPAT`.
pub(crate) fn write_mosaic_u16(path: &Path, mosaic: &CfaImage) {
    let shape = vec![mosaic.data().width(), mosaic.data().height()];
    let image = Image::from_u16(shape, &quantize(mosaic.data().pixels())).unwrap();
    let mut header = Header::new();
    if let CfaType::Bayer(pattern) = mosaic.cfa_type() {
        header.set("BAYERPAT", pattern.bayerpat()).unwrap();
    }
    write_fits(path, &image, Some(&header));
}

/// `samples` in `[0, 1]` to the 16-bit full scale, rounded.
#[expect(clippy::cast_sign_loss, reason = "the samples are in [0, 1]")]
fn quantize(samples: &[f32]) -> Vec<u16> {
    samples
        .iter()
        .map(|&sample| (sample * f32::from(u16::MAX)).round() as u16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::io::fits::decode;
    use crate::io::load_context::LoadContext;
    use crate::math::size2us::Size2us;

    /// Each file loads back as what was written, to within half the 16-bit step `0.5 / 65535` for
    /// the integer files and exactly for the float ones.
    #[test]
    fn each_file_loads_back_as_written() {
        let size = Size2us::new(60, 40);
        let field = Field::Typical;
        let dir = TempDir::new("lumos-bench-files-test");
        let within_half_step = |loaded: &[f32], expected: &[f32]| {
            assert_eq!(loaded.len(), expected.len());
            for (&loaded, &expected) in loaded.iter().zip(expected) {
                let error = (f64::from(loaded) - f64::from(expected)).abs();
                assert!(error <= 0.5 / 65_535.0 + 1e-7, "{loaded} for {expected}");
            }
        };

        let mono = field.light(size, CfaType::Mono);
        let path = dir.join("mono.fits");
        write_mosaic_u16(&path, &mono);
        let loaded = decode::load_linear(&path, &LoadContext::default()).unwrap();
        within_half_step(loaded.channel(0).pixels(), mono.data().pixels());

        let render = field.render(size).image;
        let path = dir.join("render.fits");
        write_mono_f32(&path, &render);
        let loaded = decode::load_linear(&path, &LoadContext::default()).unwrap();
        assert_eq!(loaded.channel(0).pixels(), render.channel(0).pixels());

        let mosaic = field.light(size, CfaType::Bayer(CfaPattern::Rggb));
        let path = dir.join("mosaic.fits");
        write_mosaic_u16(&path, &mosaic);
        let loaded = decode::load_cfa(&path, &LoadContext::default()).unwrap();
        assert_eq!(loaded.cfa_type(), mosaic.cfa_type());
        within_half_step(loaded.data().pixels(), mosaic.data().pixels());

        let calibrated = field.mosaic_exposure(size, CfaType::Bayer(CfaPattern::Rggb), 1);
        let path = dir.join("calibrated.fits");
        writer::cfa_fits(&calibrated, &path).unwrap();
        let loaded = decode::load_cfa(&path, &LoadContext::default()).unwrap();
        assert_eq!(loaded.cfa_type(), calibrated.cfa_type());
        assert_eq!(loaded.data().pixels(), calibrated.data().pixels());
    }
}
