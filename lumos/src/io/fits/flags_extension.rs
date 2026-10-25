//! The `LUMFLAGS` image extension that keeps an image's [`PixelFlags`] beside it in a FITS file:
//! a `BITPIX = 8` image of one byte per pixel, bit `i` the flag `1 << i`, after the HST and JWST
//! `DQ` arrays.
//!
//! The byte holds every flag, [`QualityFlags::NO_DATA`] too, so a reader of the extension alone
//! sees all of them. The image's NaN samples state the same `NO_DATA` pixels, and a read refuses
//! the file when the two disagree. A null's [`QualityFlags::REPAIRED`] stays out: the file holds
//! NaN there, not the repair, and a reader fills it again. An image whose flags hold nothing but
//! `NO_DATA` needs no extension: its NaNs carry all of it.

use common::{Buffer2, par};
use fits_well::header::{Header, Keyword};
use fits_well::image::Scaling;
use fits_well::io::Hdu;
use rayon::prelude::*;

use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::io::error::WriteError;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::extension_claims::{ExtensionClaims, IMAGE_KEYWORD, PRIMARY};
use crate::io::fits::lumos_extension::{DescribedImage, LumosExtension};
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD};
use crate::io::fits::plane_hdu::{FlagBytes, PlaneHdu};
use common::{Size2us, Vec2us};

pub(crate) const FLAGS_EXTNAME: &str = "LUMFLAGS";
pub(super) const FLAGS_FORMAT: &str = "PIXFLAGS";
pub(super) const FLAGS_VERSION: i64 = 1;

/// The extension for `flags` of the image HDU named `image_name`, numbered `extver` among the
/// file's flag extensions; `None` when the NaNs carry every flag.
pub(crate) fn encode<'a>(
    flags: Option<&'a PixelFlags>,
    image_name: Option<&str>,
    extver: i64,
) -> Result<Option<PlaneHdu<FlagBytes<'a>>>, WriteError> {
    let Some(flags) = flags.filter(|flags| flags.contains_other_than(QualityFlags::NO_DATA)) else {
        return Ok(None);
    };
    if par::iter(flags.bytes())
        .all(|&byte| FlagBytes::stored(byte) & !QualityFlags::NO_DATA.byte() == 0)
    {
        return Ok(None);
    }
    let mut header = Header::new();
    header
        .set("EXTNAME", FLAGS_EXTNAME)?
        .set("EXTVER", extver)?
        .set(IMAGE_KEYWORD, image_name.unwrap_or(PRIMARY))?
        .set(FORMAT_KEYWORD, FLAGS_FORMAT)?
        .set(VERSION_KEYWORD, FLAGS_VERSION)?;
    for (flag, name) in QualityFlags::NAMED {
        header.set(bit_keyword(flag)?, name)?;
    }
    let size = flags.size();
    Ok(Some(PlaneHdu {
        header,
        shape: [size.width, size.height].into_iter().collect(),
        scaling: Scaling::IDENTITY,
        source: FlagBytes(flags),
    }))
}

/// The flags extension of the image at HDU `image`, checked to fit `described`; `None` when
/// it has none. Refused when the file's claims do not pass [`check_claims`], or the
/// extension is not its format's ([`LumosExtension::check`]).
pub(crate) fn locate(
    file: &[Hdu],
    image: usize,
    described: &DescribedImage,
) -> Result<Option<usize>, FitsLoadError> {
    let found = check_claims(file)?.for_image(file, image)?;
    if let Some(idx) = found {
        LumosExtension::Flags.check(&file[idx], described)?;
    }
    Ok(found)
}

/// Each flags extension of the file and the image it is for, refused unless each names an
/// image of the file that no other names: one that names none would be dropped unseen.
pub(crate) fn check_claims(file: &[Hdu]) -> Result<ExtensionClaims, FitsLoadError> {
    ExtensionClaims::of(file, LumosExtension::Flags)
}

/// The flags `stored` holds — the data of the HDU [`locate`] found — joined with those
/// the decode of the image found, `decoded`.
///
/// Refused when a byte holds a bit no flag has, or when its `NO_DATA` disagrees with the
/// image's NaNs: either file was not the one Lumos wrote.
pub(crate) fn join(
    stored: Buffer2<u8>,
    decoded: Option<&PixelFlags>,
) -> Result<Option<PixelFlags>, FitsLoadError> {
    let size = stored.size();
    let mut bytes = stored;
    let unknown = !QualityFlags::KNOWN.byte();
    let no_data = QualityFlags::NO_DATA.byte();
    let disagreement = par::iter(&bytes).enumerate().find_first(|&(idx, &byte)| {
        let decoded_no_data =
            decoded.is_some_and(|flags| flags.at(idx).intersects(QualityFlags::NO_DATA));
        byte & unknown != 0 || (byte & no_data != 0) != decoded_no_data
    });
    if let Some((idx, &byte)) = disagreement {
        let pixel = Vec2us::new(idx % size.width, idx / size.width);
        let reason = if byte & unknown != 0 {
            FitsRefusal::UnknownFlagBits { pixel, byte }
        } else {
            FitsRefusal::NoDataDisagrees { pixel }
        };
        return Err(FitsLoadError::from(reason));
    }
    if let Some(decoded) = decoded {
        par::iter_mut(&mut bytes)
            .zip(decoded.bytes())
            .for_each(|(byte, &found)| *byte |= found);
    }
    Ok(PixelFlags::from_bytes(bytes))
}

/// The bytes of a flags HDU [`locate`] found, as read, as the plane of the `size` image
/// they are for.
pub(crate) fn plane(bytes: Vec<u8>, size: Size2us) -> Buffer2<u8> {
    Buffer2::new(size, bytes)
}

/// The keyword that names the flag at its bit `i`: `LUMFBi`.
fn bit_keyword(flag: QualityFlags) -> fits_well::Result<Keyword> {
    const FLAG_BIT: Keyword = Keyword::new("LUMFB");
    let bit = usize::try_from(flag.byte().trailing_zeros()).expect("a bit of a byte");
    FLAG_BIT.indexed(bit)
}

#[cfg(test)]
mod tests {

    use crate::image::pixel_flags::PixelFlags;
    use crate::image::quality_flags::QualityFlags;
    use crate::internals::fits::fits_refusal;
    use crate::io::fits::error::FitsRefusal;
    use crate::io::fits::flags_extension;
    use common::{Size2us, Vec2us};

    /// The stored flags join those the decode found, bit by bit. Stored, row-major over 3×2:
    /// nothing, `NO_DATA`, `SATURATED | DEFECT` (6), nothing, `COSMIC_RAY | REPAIRED` (24),
    /// `FLAT_FLOOR` (32). Decoded: `NO_DATA` at 1 from the NaN, `SATURATED` (2) at 3 from
    /// `DATAMAX`. Joined: 0, 1, 6, 2, 24, 32. With no decoded flags, a plane of zeros is no flags
    /// at all, and a byte past the known bits — 0x80 here — is refused.
    #[test]
    fn stored_flags_join_the_decoded_ones() {
        let size = Size2us::new(3, 2);
        let decoded = PixelFlags::from_fn(size, |idx| match idx {
            1 => QualityFlags::NO_DATA,
            3 => QualityFlags::SATURATED,
            _ => QualityFlags::default(),
        });
        let plane = |bytes| flags_extension::plane(bytes, size);
        let joined = flags_extension::join(plane(vec![0, 1, 6, 0, 24, 32]), decoded.as_ref())
            .unwrap()
            .unwrap();
        assert_eq!(joined.bytes(), &[0, 1, 6, 2, 24, 32]);
        assert_eq!(joined.count(QualityFlags::SATURATED), 2);

        assert!(
            flags_extension::join(plane(vec![0; 6]), None)
                .unwrap()
                .is_none()
        );
        let reason = fits_refusal(
            flags_extension::join(plane(vec![0, 0, 0, 0, 0x80, 0]), None).unwrap_err(),
        );
        assert_eq!(
            reason,
            FitsRefusal::UnknownFlagBits {
                pixel: Vec2us::new(1, 1),
                byte: 0x80
            }
        );
        assert_eq!(
            reason.to_string(),
            "LUMFLAGS at (1, 1): byte 0x80 holds a bit no flag has"
        );
    }

    /// Flags that hold nothing but `NO_DATA`, or no flags, need no extension: the NaNs say it all.
    /// Nor do repaired nulls, whose repair the NaN replaces.
    #[test]
    fn only_flags_past_no_data_need_an_extension() {
        let size = Size2us::new(2, 1);
        for null in [
            QualityFlags::NO_DATA,
            QualityFlags::NO_DATA.union(QualityFlags::REPAIRED),
        ] {
            let nulls = PixelFlags::from_fn(size, |_| null);
            assert!(
                flags_extension::encode(nulls.as_ref(), None, 1)
                    .unwrap()
                    .is_none(),
                "{null:?}"
            );
        }
        assert!(flags_extension::encode(None, None, 1).unwrap().is_none());
        let defect = PixelFlags::from_fn(size, |idx| {
            if idx == 0 {
                QualityFlags::DEFECT
            } else {
                QualityFlags::default()
            }
        });
        let encoded = flags_extension::encode(defect.as_ref(), Some("MASTER_DARK"), 2)
            .unwrap()
            .unwrap();
        assert_eq!(encoded.header.get_integer("EXTVER").unwrap(), Some(2));
        assert_eq!(
            encoded.header.get_text("LUMFOR").unwrap(),
            Some("MASTER_DARK")
        );
        assert_eq!(encoded.header.get_text("LUMFB2").unwrap(), Some("DEFECT"));
        assert_eq!(
            encoded.header.get_text("LUMFB5").unwrap(),
            Some("FLAT_FLOOR")
        );
    }
}
