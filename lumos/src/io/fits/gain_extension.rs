//! [`GainExtension`]: the `LUMGAIN` image extension that keeps an image's [`FlatGain`] beside it
//! in a FITS file.

use arrayvec::ArrayVec;
use fits_well::header::Header;
use fits_well::image::{Data as FitsData, Image};
use fits_well::io::Hdu;
use imaginarium::Buffer2;

use crate::image::flat_gain::{self, FlatGain, GainGrid};
use crate::io::error::WriteError;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::extension_claims::{ExtensionClaims, IMAGE_KEYWORD, PRIMARY};
use crate::io::fits::lumos_extension::{DescribedImage, LumosExtension};
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD};
use crate::math::size2us::Size2us;

pub(super) const GAIN_EXTNAME: &str = "LUMGAIN";
pub(super) const GAIN_FORMAT: &str = "FLATGAIN";
pub(super) const GAIN_VERSION: i64 = 1;
/// The keyword that states the pixels between nodes, which a reader must share.
const STEP_KEYWORD: &str = "LUMGSTEP";

/// The flat gain of one image as a `BITPIX = −32` image extension of shape
/// `[cols, rows, channels]`: each color's or channel's node grid, as [`FlatGain`] holds it.
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
    ) -> Result<Option<Self>, WriteError> {
        let Some(gain) = gain else {
            return Ok(None);
        };
        let mut header = Header::new();
        header
            .set("EXTNAME", GAIN_EXTNAME)
            .and_then(|header| header.set(IMAGE_KEYWORD, image_name.unwrap_or(PRIMARY)))
            .and_then(|header| header.set(FORMAT_KEYWORD, GAIN_FORMAT))
            .and_then(|header| header.set(VERSION_KEYWORD, GAIN_VERSION))
            .and_then(|header| header.set(STEP_KEYWORD, flat_gain::STEP as i64))?;
        let grid = GainGrid::of(gain.size());
        let nodes: Vec<f32> = gain.planes().flatten().copied().collect();
        let image = Image::new([grid.cols, grid.rows, gain.channels()], nodes)?;
        Ok(Some(Self { image, header }))
    }

    /// The gain extension of the image at HDU `image`, checked to fit `described`; `None` when it
    /// has none. Refused when a gain extension names no image of the file, two name one, it is not
    /// its format's ([`LumosExtension::check`]), or its nodes are spaced other than this reader's.
    pub(crate) fn locate(
        file: &[Hdu],
        image: usize,
        described: &DescribedImage,
    ) -> Result<Option<usize>, FitsLoadError> {
        let found = ExtensionClaims::of(file, LumosExtension::Gain)?.for_image(file, image)?;
        if let Some(idx) = found {
            let hdu = &file[idx];
            LumosExtension::Gain.check(hdu, described)?;
            let step = hdu.header.get_integer(STEP_KEYWORD)?;
            if step != Some(flat_gain::STEP as i64) {
                return Err(FitsRefusal::GainStep { step }.into());
            }
        }
        Ok(found)
    }

    /// The gain `stored` holds — the data of the HDU [`Self::locate`] found — over a `size` image.
    /// Refused when a node is not a finite gain above zero: no flat multiplies by that.
    pub(crate) fn decode(stored: FitsData, size: Size2us) -> Result<FlatGain, FitsLoadError> {
        let FitsData::F32(nodes) = stored else {
            unreachable!("`check` admitted only BITPIX = -32");
        };
        let grid = GainGrid::of(size);
        let per_plane = grid.cols * grid.rows;
        debug_assert_eq!(nodes.len() % per_plane, 0);
        if let Some(idx) = nodes
            .iter()
            .position(|&gain| !gain.is_finite() || gain <= 0.0)
        {
            return Err(FitsLoadError::from(FitsRefusal::NotAGain {
                node: idx,
                value: nodes[idx],
            }));
        }
        let planes: ArrayVec<Buffer2<f32>, 3> = nodes
            .chunks(per_plane)
            .map(|plane| Buffer2::new(grid.cols, grid.rows, plane.to_vec()))
            .collect();
        Ok(FlatGain::from_nodes(size, planes))
    }
}
