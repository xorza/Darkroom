//! [`GainExtension`]: the `LUMGAIN` image extension that keeps an image's [`FlatGain`] beside it
//! in a FITS file.

use std::path::Path;

use arrayvec::ArrayVec;
use fits_well::header::Header;
use fits_well::image::{Bitpix, Data as FitsData, Image, Scaling};
use fits_well::io::{Hdu, HduKind};
use imaginarium::Buffer2;

use crate::image::flat_gain::{self, FlatGain, GainGrid};
use crate::io::error::{FitsRefusal, FitsWriteError, ImageError};
use crate::io::fits::extension_claims::{ExtensionClaims, IMAGE_KEYWORD, PRIMARY};
use crate::io::fits::hdu_list::HduList;
use crate::io::fits::lumos_extension::LumosExtension;
use crate::math::size2us::Size2us;

pub(crate) const GAIN_EXTNAME: &str = "LUMGAIN";
pub(crate) const GAIN_FORMAT: &str = "FLATGAIN";
pub(crate) const GAIN_VERSION: i64 = 1;
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
    ) -> Result<Option<Self>, FitsWriteError> {
        let Some(gain) = gain else {
            return Ok(None);
        };
        let mut header = Header::new();
        header
            .set("EXTNAME", GAIN_EXTNAME)
            .and_then(|header| header.set(IMAGE_KEYWORD, image_name.unwrap_or(PRIMARY)))
            .and_then(|header| header.set("LUMOSFMT", GAIN_FORMAT))
            .and_then(|header| header.set("LUMOSVER", GAIN_VERSION))
            .and_then(|header| header.set(STEP_KEYWORD, flat_gain::STEP as i64))?;
        let grid = GainGrid::of(gain.size());
        let nodes: Vec<f32> = gain.planes().flatten().copied().collect();
        let image = Image::new([grid.columns, grid.rows, gain.channels()], nodes)?;
        Ok(Some(Self { image, header }))
    }

    /// The gain extension of the image at HDU `image`, checked to fit a `size` image; `None` when
    /// it has none. Refused when a gain extension names no image of the file, two name one, or it
    /// is not this version's.
    pub(crate) fn locate(
        file: HduList<'_>,
        image: usize,
        size: Size2us,
    ) -> Result<Option<usize>, ImageError> {
        let found = ExtensionClaims::of(file, LumosExtension::Gain)?.for_image(file, image)?;
        if let Some(index) = found {
            Self::check(file.path, &file.hdus[index], size)?;
        }
        Ok(found)
    }

    /// Refuse a gain HDU that is not this format's: another version or step, another type or
    /// scaling, or another grid than the `size` image's, of other than one or three planes.
    fn check(path: &Path, hdu: &Hdu, size: Size2us) -> Result<(), ImageError> {
        let extension = LumosExtension::Gain;
        let refuse = |reason| ImageError::fits_unsupported(path, reason);
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
            return Err(refuse(FitsRefusal::ExtensionFormat {
                extension,
                format: format.map(str::to_owned),
                version,
            }));
        }
        let step = integer(STEP_KEYWORD)?;
        if step != Some(flat_gain::STEP as i64) {
            return Err(refuse(FitsRefusal::GainStep { step }));
        }
        if hdu.kind != HduKind::Image {
            return Err(refuse(FitsRefusal::ExtensionNotImage { extension }));
        }
        let image = hdu
            .image()
            .map_err(|source| ImageError::fits(path, source))?;
        if image.bitpix != Bitpix::F32 || image.scaling != Scaling::IDENTITY {
            return Err(refuse(FitsRefusal::ExtensionSamples {
                extension,
                bitpix: image.bitpix,
                scaling: image.scaling,
            }));
        }
        let grid = GainGrid::of(size);
        if !matches!(image.shape, [columns, rows, 1 | 3] if *columns == grid.columns && *rows == grid.rows)
        {
            return Err(refuse(FitsRefusal::ExtensionShape {
                extension,
                shape: image.shape.to_vec(),
                image: size,
            }));
        }
        Ok(())
    }

    /// The gain `stored` holds — the data of the HDU [`Self::locate`] found — over a `size` image.
    /// Refused when a node is not a finite gain above zero: no flat multiplies by that.
    pub(crate) fn decode(
        path: &Path,
        stored: FitsData,
        size: Size2us,
    ) -> Result<FlatGain, ImageError> {
        let FitsData::F32(nodes) = stored else {
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
                FitsRefusal::NotAGain {
                    node: index,
                    value: nodes[index],
                },
            ));
        }
        let planes: ArrayVec<Buffer2<f32>, 3> = nodes
            .chunks(per_plane)
            .map(|plane| Buffer2::new(grid.columns, grid.rows, plane.to_vec()))
            .collect();
        Ok(FlatGain::from_nodes(size, planes))
    }
}
