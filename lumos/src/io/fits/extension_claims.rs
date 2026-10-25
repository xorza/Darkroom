//! [`ExtensionClaims`]: the Lumos image extensions of a FITS file that describe another of its
//! images, and the image each is for.

use fits_well::header::Keyword;
use fits_well::io::Hdu;

use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::lumos_extension::LumosExtension;
use crate::io::fits::metadata;

/// The keyword that names the image HDU an extension is for: its `EXTNAME`, or [`PRIMARY`].
pub(crate) const IMAGE_KEYWORD: Keyword = Keyword::new("LUMFOR");
/// The name of a primary HDU without an `EXTNAME`, as astropy and fitsio name it.
pub(crate) const PRIMARY: &str = "PRIMARY";

/// The extensions of one kind in a file, each with the image it names, checked so that each names
/// an image of the file no other names: one that names none would be dropped unseen.
#[derive(Debug)]
pub(crate) struct ExtensionClaims {
    claims: Vec<Claim>,
}

/// One extension, and the name of the image it says it is for.
#[derive(Debug)]
struct Claim {
    hdu: usize,
    image: String,
}

impl ExtensionClaims {
    /// The `extension` HDUs of `hdus`, a file's, refused unless each names one image of the file
    /// that no other names.
    pub(crate) fn of(hdus: &[Hdu], extension: LumosExtension) -> Result<Self, FitsLoadError> {
        let mut images = Vec::new();
        let mut claims = Vec::new();
        for (idx, hdu) in hdus.iter().enumerate() {
            if extension.names(hdu)? {
                let image = metadata::read_text(&hdu.header, IMAGE_KEYWORD)?.ok_or_else(|| {
                    FitsLoadError::from(FitsRefusal::ClaimsNoImage {
                        extension,
                        hdu: idx,
                    })
                })?;
                claims.push(Claim { hdu: idx, image });
            } else if Self::holds_image(hdu)? {
                images.push(hdu);
            }
        }
        for (pos, claim) in claims.iter().enumerate() {
            if !names_any(&images, &claim.image)? {
                return Err(FitsLoadError::from(FitsRefusal::ClaimsMissingImage {
                    extension,
                    hdu: claim.hdu,
                    image: claim.image.clone(),
                }));
            }
            if claims[..pos]
                .iter()
                .any(|earlier| earlier.image.eq_ignore_ascii_case(&claim.image))
            {
                return Err(FitsLoadError::from(FitsRefusal::DuplicateClaim {
                    extension,
                    image: claim.image.clone(),
                }));
            }
        }
        Ok(Self { claims })
    }

    /// The extension for the image at HDU `image`; `None` when it has none.
    pub(crate) fn for_image(
        &self,
        hdus: &[Hdu],
        image: usize,
    ) -> Result<Option<usize>, FitsLoadError> {
        for claim in &self.claims {
            if hdus[image].matches(claim.image.as_str())? {
                return Ok(Some(claim.hdu));
            }
        }
        Ok(None)
    }

    /// Whether `hdu` is an image of its own: an image HDU with at least one axis, which no lumos
    /// extension that describes another image is. A header-only image extension (`NAXIS = 0`) is
    /// none; one with a zero-length axis is, and is refused for that when it is read.
    pub(super) fn holds_image(hdu: &Hdu) -> Result<bool, FitsLoadError> {
        Ok(hdu.is_image() && !Self::describes_another(hdu)?)
    }

    /// Whether `hdu` is an extension that describes another image rather than being one.
    fn describes_another(hdu: &Hdu) -> Result<bool, FitsLoadError> {
        for extension in LumosExtension::ALL {
            if extension.names(hdu)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Whether `name` names one of `images`: as an `EXTNAME`, or [`PRIMARY`] for an unnamed
/// primary HDU, as [`Hdu::matches`] reads it.
fn names_any(images: &[&Hdu], name: &str) -> Result<bool, FitsLoadError> {
    for hdu in images {
        if hdu.matches(name)? {
            return Ok(true);
        }
    }
    Ok(false)
}
