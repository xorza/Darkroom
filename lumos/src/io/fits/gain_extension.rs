//! The `LUMGAIN` image extension that keeps an image's [`FlatGain`] beside it in a FITS file: a
//! `BITPIX = −32` image of shape `[cols, rows, channels]`, each color's or channel's node grid as
//! [`FlatGain`] holds it.
//!
//! Without it, a frame a flat divided reloads with its record of the division and no gain, and
//! its noise model would read every pixel as undivided.

use arrayvec::ArrayVec;
use common::Buffer2;
use fits_well::header::{Header, Keyword};
use fits_well::image::Scaling;
use fits_well::io::Hdu;

use crate::image::flat_gain::gain_key::GainKey;
use crate::image::flat_gain::{self, FlatGain, GainGrid};
use crate::io::error::WriteError;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::extension_claims::{ExtensionClaims, IMAGE_KEYWORD, PRIMARY};
use crate::io::fits::lumos_extension::{DescribedImage, LumosExtension};
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD};
use crate::io::fits::plane_hdu::{FloatPlanes, PlaneHdu};
use crate::math::size2us::Size2us;

pub(super) const GAIN_EXTNAME: &str = "LUMGAIN";
pub(super) const GAIN_FORMAT: &str = "FLATGAIN";
pub(super) const GAIN_VERSION: i64 = 1;
/// The keyword that states the pixels between nodes, which a reader must share.
const STEP_KEYWORD: Keyword = Keyword::new("LUMGSTEP");
/// The keyword that states the gain's [`GainKey`], so a reader that holds that gain already reads
/// no node. A file without it is read whole.
const KEY_KEYWORD: Keyword = Keyword::new("LUMGSUM");

/// A gain extension [`locate`] found: its HDU, and the key its header states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocatedGain {
    pub(crate) hdu: usize,
    pub(crate) key: Option<GainKey>,
}

/// The extension for `gain` of the image HDU named `image_name`; `None` for an image no flat
/// divided.
pub(crate) fn encode<'a>(
    gain: Option<&'a FlatGain>,
    image_name: Option<&str>,
) -> Result<Option<PlaneHdu<FloatPlanes<'a>>>, WriteError> {
    let Some(gain) = gain else {
        return Ok(None);
    };
    let mut header = Header::new();
    header
        .set("EXTNAME", GAIN_EXTNAME)?
        .set(IMAGE_KEYWORD, image_name.unwrap_or(PRIMARY))?
        .set(FORMAT_KEYWORD, GAIN_FORMAT)?
        .set(VERSION_KEYWORD, GAIN_VERSION)?
        .set(STEP_KEYWORD, flat_gain::STEP as i64)?
        .set(KEY_KEYWORD, gain.key().to_string())?;
    let grid = GainGrid::of(gain.size());
    Ok(Some(PlaneHdu {
        header,
        shape: [grid.cols, grid.rows, gain.channels()]
            .into_iter()
            .collect(),
        scaling: Scaling::IDENTITY,
        source: FloatPlanes::new(gain.planes().collect()),
    }))
}

/// The gain extension of the image at HDU `image`, checked to fit `described`, with the key it
/// states; `None` when it has none. Refused when a gain extension names no image of the file, two
/// name one, it is not its format's ([`LumosExtension::check`]), its nodes are spaced other than
/// this reader's, or it states a key that is no 64 hex digits.
pub(crate) fn locate(
    file: &[Hdu],
    image: usize,
    described: &DescribedImage,
) -> Result<Option<LocatedGain>, FitsLoadError> {
    let Some(hdu) = ExtensionClaims::of(file, LumosExtension::Gain)?.for_image(file, image)? else {
        return Ok(None);
    };
    let header = &file[hdu].header;
    LumosExtension::Gain.check(&file[hdu], described)?;
    let step = header.get_integer(STEP_KEYWORD)?;
    if step != Some(flat_gain::STEP as i64) {
        return Err(FitsRefusal::GainStep { step }.into());
    }
    let key = header
        .get_text(KEY_KEYWORD)?
        .map(|text| {
            GainKey::from_hex(text).ok_or_else(|| FitsRefusal::GainKeyText {
                text: text.to_owned(),
            })
        })
        .transpose()?;
    Ok(Some(LocatedGain { hdu, key }))
}

/// The gain `nodes` hold — each channel's plane of the HDU [`locate`] found — over a
/// `size` image. Refused when a node is not a finite gain above zero: no flat multiplies by
/// that, or when the gain is not the one the `stated` key names. A node is named by its index
/// over the channels' planes in turn, as the HDU stores it.
pub(crate) fn decode(
    nodes: ArrayVec<Vec<f32>, 3>,
    size: Size2us,
    stated: Option<GainKey>,
) -> Result<FlatGain, FitsLoadError> {
    let grid = GainGrid::of(size);
    let per_plane = grid.nodes();
    debug_assert!(nodes.iter().all(|plane| plane.len() == per_plane));
    if let Some((idx, &value)) = nodes
        .iter()
        .flatten()
        .enumerate()
        .find(|&(_, &gain)| !gain.is_finite() || gain <= 0.0)
    {
        return Err(FitsLoadError::from(FitsRefusal::NotAGain {
            node: idx,
            value,
        }));
    }
    let planes = nodes
        .into_iter()
        .map(|plane| Buffer2::new(grid.cols, grid.rows, plane))
        .collect();
    let gain = FlatGain::from_nodes(size, planes);
    if stated.is_some_and(|stated| stated != gain.key()) {
        return Err(FitsRefusal::GainKeyMismatch.into());
    }
    Ok(gain)
}
