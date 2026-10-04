//! [`FlagsExtension`]: the `LUMFLAGS` image extension that keeps an image's [`PixelFlags`] beside
//! it in a FITS file.

use std::io;
use std::path::Path;

use fits_well::header::Header;
use fits_well::image::{Bitpix, Image, ImageData, Scaling};
use fits_well::io::{Hdu, HduKind};
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::error::ImageError;
use crate::io::image::fits::error::fits_to_io;
use crate::io::image::fits::metadata::read_text;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::math::size2us::Size2us;

pub(crate) const FLAGS_EXTNAME: &str = "LUMFLAGS";
const FLAGS_FORMAT: &str = "PIXFLAGS";
const FLAGS_VERSION: i64 = 1;
/// The keyword that names the image HDU the flags are for: its `EXTNAME`, or [`PRIMARY`].
const IMAGE_KEYWORD: &str = "LUMFOR";
/// The name of a primary HDU without an `EXTNAME`, as astropy and fitsio name it.
const PRIMARY: &str = "PRIMARY";

/// The flags of one image as a `BITPIX = 8` image extension: one byte per pixel, bit `i` the flag
/// `1 << i`, after the HST and JWST `DQ` arrays.
///
/// The byte holds every flag, [`QualityFlags::NO_DATA`] too, so a reader of the extension alone
/// sees all of them. The image's NaN samples state the same `NO_DATA` pixels, and a read refuses
/// the file when the two disagree. An image whose flags hold nothing but `NO_DATA` needs no
/// extension: its NaNs carry all of it.
#[derive(Debug)]
pub(crate) struct FlagsExtension {
    pub(crate) image: Image,
    pub(crate) header: Header,
}

/// A flags extension, and the name of the image it says it is for.
#[derive(Debug)]
pub(crate) struct Claim {
    hdu: usize,
    image: String,
}

impl FlagsExtension {
    /// The extension for `flags` of the image HDU named `image_name`, numbered `extver` among the
    /// file's flag extensions; `None` when the NaNs carry every flag.
    pub(crate) fn encode(
        flags: Option<&PixelFlags>,
        image_name: Option<&str>,
        extver: i64,
    ) -> io::Result<Option<Self>> {
        let Some(flags) = flags.filter(|flags| flags.contains_other_than(QualityFlags::NO_DATA))
        else {
            return Ok(None);
        };
        let mut header = Header::new();
        header
            .set("EXTNAME", FLAGS_EXTNAME)
            .and_then(|header| header.set("EXTVER", extver))
            .and_then(|header| header.set(IMAGE_KEYWORD, image_name.unwrap_or(PRIMARY)))
            .and_then(|header| header.set("LUMOSFMT", FLAGS_FORMAT))
            .and_then(|header| header.set("LUMOSVER", FLAGS_VERSION))
            .map_err(fits_to_io)?;
        for (flag, name) in QualityFlags::NAMED {
            header.set(&bit_keyword(flag), name).map_err(fits_to_io)?;
        }
        let size = flags.size();
        let image =
            Image::new([size.width, size.height], flags.bytes().to_vec()).map_err(fits_to_io)?;
        Ok(Some(Self { image, header }))
    }

    /// Whether `hdu` is a flags extension, which describes another image rather than being one.
    pub(crate) fn describes_another(path: &Path, hdu: &Hdu) -> Result<bool, ImageError> {
        Ok(read_text(&hdu.header, "EXTNAME")
            .map_err(|source| ImageError::fits(path, source))?
            .is_some_and(|extname| extname.eq_ignore_ascii_case(FLAGS_EXTNAME)))
    }

    /// The flags extension of the image at HDU `image`, checked to fit a `size` image; `None`
    /// when it has none. Refused when the file's claims do not pass [`Self::check_claims`], or
    /// the extension is not this version's.
    pub(crate) fn locate(
        path: &Path,
        hdus: &[Hdu],
        image: usize,
        size: Size2us,
    ) -> Result<Option<usize>, ImageError> {
        let claims = Self::check_claims(path, hdus)?;
        let Some(name) = image_name(path, hdus, image)? else {
            return Ok(None);
        };
        let found = claims
            .iter()
            .find(|claim| claim.image.eq_ignore_ascii_case(&name))
            .map(|claim| claim.hdu);
        if let Some(index) = found {
            Self::check(path, &hdus[index], size)?;
        }
        Ok(found)
    }

    /// Each flags extension of the file and the image it is for, refused unless each names an
    /// image of the file that no other names: one that names none would be dropped unseen.
    pub(crate) fn check_claims(path: &Path, hdus: &[Hdu]) -> Result<Vec<Claim>, ImageError> {
        let mut images = Vec::new();
        let mut claims = Vec::new();
        for (index, hdu) in hdus.iter().enumerate() {
            if Self::describes_another(path, hdu)? {
                let image = read_text(&hdu.header, IMAGE_KEYWORD)
                    .map_err(|source| ImageError::fits(path, source))?
                    .ok_or_else(|| {
                        ImageError::fits_unsupported(
                            path,
                            format!("{FLAGS_EXTNAME} HDU {index} names no image"),
                        )
                    })?;
                claims.push(Claim { hdu: index, image });
            } else if hdu.is_image()
                && hdu.data_bytes > 0
                && let Some(name) = image_name(path, hdus, index)?
            {
                images.push(name);
            }
        }
        for (position, claim) in claims.iter().enumerate() {
            let same = |name: &String| name.eq_ignore_ascii_case(&claim.image);
            if !images.iter().any(same) {
                return Err(ImageError::fits_unsupported(
                    path,
                    format!(
                        "{FLAGS_EXTNAME} HDU {} is for {:?}, which is no image of the file",
                        claim.hdu, claim.image
                    ),
                ));
            }
            if claims[..position]
                .iter()
                .any(|earlier| same(&earlier.image))
            {
                return Err(ImageError::fits_unsupported(
                    path,
                    format!("two {FLAGS_EXTNAME} HDUs claim image {:?}", claim.image),
                ));
            }
        }
        Ok(claims)
    }

    /// Refuse a flags HDU that is not this format's: another version, another type or scaling, or
    /// another geometry than the `size` image it is for.
    fn check(path: &Path, hdu: &Hdu, size: Size2us) -> Result<(), ImageError> {
        let refuse =
            |reason: &str| ImageError::fits_unsupported(path, format!("{FLAGS_EXTNAME}: {reason}"));
        let header = &hdu.header;
        let format = header
            .get_text("LUMOSFMT")
            .map_err(|source| ImageError::fits(path, source))?;
        let version = header
            .get_integer("LUMOSVER")
            .map_err(|source| ImageError::fits(path, source))?;
        if format != Some(FLAGS_FORMAT) || version != Some(FLAGS_VERSION) {
            return Err(refuse(&format!(
                "format {format:?} version {version:?}; expected {FLAGS_FORMAT} version \
                 {FLAGS_VERSION}"
            )));
        }
        if hdu.kind != HduKind::Image {
            return Err(refuse("not an uncompressed image extension"));
        }
        let image = hdu
            .image()
            .map_err(|source| ImageError::fits(path, source))?;
        if image.bitpix != Bitpix::U8 || image.scaling != Scaling::IDENTITY {
            return Err(refuse("not unscaled BITPIX = 8 bytes"));
        }
        if image.shape != [size.width, size.height] {
            return Err(refuse(&format!(
                "shape {:?} is not the image's {}x{}",
                image.shape, size.width, size.height
            )));
        }
        Ok(())
    }

    /// The flags `stored` holds — the data of the HDU [`Self::locate`] found — joined with those
    /// the decode of the image found, `decoded`.
    ///
    /// Refused when a byte holds a bit no flag has, or when its `NO_DATA` disagrees with the
    /// image's NaNs: either file was not the one Lumos wrote.
    pub(crate) fn join(
        path: &Path,
        stored: ImageData,
        size: Size2us,
        decoded: Option<&PixelFlags>,
    ) -> Result<Option<PixelFlags>, ImageError> {
        let ImageData::U8(mut bytes) = stored else {
            unreachable!("`check` admitted only BITPIX = 8");
        };
        debug_assert_eq!(bytes.len(), size.pixel_count());
        let unknown = !QualityFlags::KNOWN.byte();
        let no_data = QualityFlags::NO_DATA.byte();
        let disagreement = bytes.par_iter().enumerate().find_first(|&(index, &byte)| {
            let decoded_no_data =
                decoded.is_some_and(|flags| flags.at(index).intersects(QualityFlags::NO_DATA));
            byte & unknown != 0 || (byte & no_data != 0) != decoded_no_data
        });
        if let Some((index, &byte)) = disagreement {
            let (x, y) = (index % size.width, index / size.width);
            let reason = if byte & unknown != 0 {
                format!("byte {byte:#04x} holds a bit no flag has")
            } else {
                "NO_DATA disagrees with the image's NaN".to_owned()
            };
            return Err(ImageError::fits_unsupported(
                path,
                format!("{FLAGS_EXTNAME} at ({x}, {y}): {reason}"),
            ));
        }
        if let Some(decoded) = decoded {
            bytes
                .par_iter_mut()
                .zip(decoded.bytes())
                .for_each(|(byte, &found)| *byte |= found);
        }
        Ok(PixelFlags::from_buffer(Buffer2::new(
            size.width,
            size.height,
            bytes,
        )))
    }
}

/// The name a flags extension refers to the image at HDU `index` by: its `EXTNAME`, or
/// [`PRIMARY`] for an unnamed primary HDU; `None` for an unnamed extension, which none can name.
fn image_name(path: &Path, hdus: &[Hdu], index: usize) -> Result<Option<String>, ImageError> {
    let extname = read_text(&hdus[index].header, "EXTNAME")
        .map_err(|source| ImageError::fits(path, source))?;
    Ok(match extname {
        Some(extname) => Some(extname),
        None => (index == 0).then(|| PRIMARY.to_owned()),
    })
}

/// The keyword that names the flag at its bit `i`: `LUMFBi`.
fn bit_keyword(flag: QualityFlags) -> String {
    format!("LUMFB{}", flag.byte().trailing_zeros())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use fits_well::image::ImageData;

    use crate::io::image::error::ImageError;
    use crate::io::image::fits::flags_extension::FlagsExtension;
    use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
    use crate::math::size2us::Size2us;

    /// The stored flags join those the decode found, bit by bit. Stored, row-major over 3×2:
    /// nothing, `NO_DATA`, `SATURATED | DEFECT` (6), nothing, `COSMIC_RAY | REPAIRED` (24),
    /// `FLAT_FLOOR` (32). Decoded: `NO_DATA` at 1 from the NaN, `SATURATED` (2) at 3 from
    /// `DATAMAX`. Joined: 0, 1, 6, 2, 24, 32. With no decoded flags, a plane of zeros is no flags at
    /// all, and a byte past the known bits — 0x80 here — is refused.
    #[test]
    fn stored_flags_join_the_decoded_ones() {
        let size = Size2us::new(3, 2);
        let path = Path::new("flags.fits");
        let decoded = PixelFlags::from_fn(size, |index| match index {
            1 => QualityFlags::NO_DATA,
            3 => QualityFlags::SATURATED,
            _ => QualityFlags::default(),
        });
        let joined = FlagsExtension::join(
            path,
            ImageData::U8(vec![0, 1, 6, 0, 24, 32]),
            size,
            decoded.as_ref(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(joined.bytes(), &[0, 1, 6, 2, 24, 32]);
        assert_eq!(joined.count(QualityFlags::SATURATED), 2);

        assert!(
            FlagsExtension::join(path, ImageData::U8(vec![0; 6]), size, None)
                .unwrap()
                .is_none()
        );
        let error =
            FlagsExtension::join(path, ImageData::U8(vec![0, 0, 0, 0, 0x80, 0]), size, None)
                .unwrap_err();
        assert!(
            matches!(&error, ImageError::FitsUnsupported { reason, .. }
                if reason == "LUMFLAGS at (1, 1): byte 0x80 holds a bit no flag has"),
            "{error:?}"
        );
    }

    /// Flags that hold nothing but `NO_DATA`, or no flags, need no extension: the NaNs say it all.
    #[test]
    fn only_flags_past_no_data_need_an_extension() {
        let size = Size2us::new(2, 1);
        let nulls = PixelFlags::from_fn(size, |_| QualityFlags::NO_DATA);
        assert!(
            FlagsExtension::encode(nulls.as_ref(), None, 1)
                .unwrap()
                .is_none()
        );
        assert!(FlagsExtension::encode(None, None, 1).unwrap().is_none());
        let defect = PixelFlags::from_fn(size, |index| {
            if index == 0 {
                QualityFlags::DEFECT
            } else {
                QualityFlags::default()
            }
        });
        let encoded = FlagsExtension::encode(defect.as_ref(), Some("MASTER_DARK"), 2)
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
