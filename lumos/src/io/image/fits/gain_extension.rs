//! [`GainExtension`]: the `LUMGAIN` image extension that keeps an image's [`FlatGain`] beside it
//! in a FITS file.

use std::io;
use std::path::Path;

use arrayvec::ArrayVec;
use fits_well::header::Header;
use fits_well::image::{Bitpix, Image, ImageData, Scaling};
use fits_well::io::{Hdu, HduKind};
use imaginarium::Buffer2;

use crate::io::image::error::ImageError;
use crate::io::image::fits::error::fits_to_io;
use crate::io::image::fits::extension_claims::{ExtensionClaims, IMAGE_KEYWORD, PRIMARY};
use crate::io::image::flat_gain::{self, FlatGain, GainGrid};
use crate::math::size2us::Size2us;

pub(crate) const GAIN_EXTNAME: &str = "LUMGAIN";
const GAIN_FORMAT: &str = "FLATGAIN";
const GAIN_VERSION: i64 = 1;
/// The keyword that states the pixels between nodes, which a reader must share.
const STEP_KEYWORD: &str = "LUMGSTEP";

/// The flat gain of one image as a `BITPIX = −32` image extension of shape
/// `[columns, rows, channels]`: each colour's or channel's node grid, as [`FlatGain`] holds it.
///
/// Without it, a frame a flat divided reloads with its record of the division and no gain, and
/// its noise model would read every pixel as undivided.
#[derive(Debug)]
pub(crate) struct GainExtension {
    pub(crate) image: Image,
    pub(crate) header: Header,
}

impl GainExtension {
    /// The extension for `gain` of the image HDU named `image_name`; `None` for an image no flat
    /// divided.
    pub(crate) fn encode(
        gain: Option<&FlatGain>,
        image_name: Option<&str>,
    ) -> io::Result<Option<Self>> {
        let Some(gain) = gain else {
            return Ok(None);
        };
        let mut header = Header::new();
        header
            .set("EXTNAME", GAIN_EXTNAME)
            .and_then(|header| header.set(IMAGE_KEYWORD, image_name.unwrap_or(PRIMARY)))
            .and_then(|header| header.set("LUMOSFMT", GAIN_FORMAT))
            .and_then(|header| header.set("LUMOSVER", GAIN_VERSION))
            .and_then(|header| header.set(STEP_KEYWORD, flat_gain::STEP as i64))
            .map_err(fits_to_io)?;
        let grid = GainGrid::of(gain.size());
        let nodes: Vec<f32> = gain.planes().flatten().copied().collect();
        let image =
            Image::new([grid.columns, grid.rows, gain.channels()], nodes).map_err(fits_to_io)?;
        Ok(Some(Self { image, header }))
    }

    /// The gain extension of the image at HDU `image`, checked to fit a `size` image; `None` when
    /// it has none. Refused when a gain extension names no image of the file, two name one, or it
    /// is not this version's.
    pub(crate) fn locate(
        path: &Path,
        hdus: &[Hdu],
        image: usize,
        size: Size2us,
    ) -> Result<Option<usize>, ImageError> {
        let found = ExtensionClaims::of(path, hdus, GAIN_EXTNAME)?.for_image(path, hdus, image)?;
        if let Some(index) = found {
            Self::check(path, &hdus[index], size)?;
        }
        Ok(found)
    }

    /// Refuse a gain HDU that is not this format's: another version or step, another type or
    /// scaling, or another grid than the `size` image's, of other than one or three planes.
    fn check(path: &Path, hdu: &Hdu, size: Size2us) -> Result<(), ImageError> {
        let refuse =
            |reason: &str| ImageError::fits_unsupported(path, format!("{GAIN_EXTNAME}: {reason}"));
        let header = &hdu.header;
        let integer = |keyword| {
            header
                .get_integer(keyword)
                .map_err(|source| ImageError::fits(path, source))
        };
        let format = header
            .get_text("LUMOSFMT")
            .map_err(|source| ImageError::fits(path, source))?;
        let version = integer("LUMOSVER")?;
        if format != Some(GAIN_FORMAT) || version != Some(GAIN_VERSION) {
            return Err(refuse(&format!(
                "format {format:?} version {version:?}; expected {GAIN_FORMAT} version \
                 {GAIN_VERSION}"
            )));
        }
        let step = integer(STEP_KEYWORD)?;
        if step != Some(flat_gain::STEP as i64) {
            return Err(refuse(&format!(
                "a node every {step:?} pixels; this reader takes {}",
                flat_gain::STEP
            )));
        }
        if hdu.kind != HduKind::Image {
            return Err(refuse("not an uncompressed image extension"));
        }
        let image = hdu
            .image()
            .map_err(|source| ImageError::fits(path, source))?;
        if image.bitpix != Bitpix::F32 || image.scaling != Scaling::IDENTITY {
            return Err(refuse("not unscaled BITPIX = -32 floats"));
        }
        let grid = GainGrid::of(size);
        if !matches!(image.shape, [columns, rows, 1 | 3] if *columns == grid.columns && *rows == grid.rows)
        {
            return Err(refuse(&format!(
                "shape {:?} is not one or three {}x{} grids over the image's {}x{}",
                image.shape, grid.columns, grid.rows, size.width, size.height
            )));
        }
        Ok(())
    }

    /// The gain `stored` holds — the data of the HDU [`Self::locate`] found — over a `size` image.
    /// Refused when a node is not a finite gain above zero: no flat multiplies by that.
    pub(crate) fn decode(
        path: &Path,
        stored: ImageData,
        size: Size2us,
    ) -> Result<FlatGain, ImageError> {
        let ImageData::F32(nodes) = stored else {
            unreachable!("`check` admitted only BITPIX = -32");
        };
        let grid = GainGrid::of(size);
        let per_plane = grid.columns * grid.rows;
        debug_assert_eq!(nodes.len() % per_plane, 0);
        if let Some(index) = nodes
            .iter()
            .position(|&gain| !gain.is_finite() || gain <= 0.0)
        {
            return Err(ImageError::fits_unsupported(
                path,
                format!(
                    "{GAIN_EXTNAME} node {index} holds {}, not a gain",
                    nodes[index]
                ),
            ));
        }
        let planes: ArrayVec<Buffer2<f32>, 3> = nodes
            .chunks(per_plane)
            .map(|plane| Buffer2::new(grid.columns, grid.rows, plane.to_vec()))
            .collect();
        Ok(FlatGain::from_nodes(size, planes))
    }
}
