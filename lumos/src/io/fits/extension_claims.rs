//! [`ExtensionClaims`]: the Lumos image extensions of a FITS file that describe another of its
//! images, and the image each is for.

use fits_well::io::Hdu;

use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::lumos_extension::LumosExtension;
use crate::io::fits::metadata;

/// The keyword that names the image HDU an extension is for: its `EXTNAME`, or [`PRIMARY`].
pub(super) const IMAGE_KEYWORD: &str = "LUMFOR";
/// The name of a primary HDU without an `EXTNAME`, as astropy and fitsio name it.
pub(super) const PRIMARY: &str = "PRIMARY";

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
            if extension.names(&hdu.header)? {
                let image = metadata::read_text(&hdu.header, IMAGE_KEYWORD)?.ok_or_else(|| {
                    FitsLoadError::from(FitsRefusal::ClaimsNoImage {
                        extension,
                        hdu: idx,
                    })
                })?;
                claims.push(Claim { hdu: idx, image });
            } else if Self::holds_image(hdu)?
                && let Some(name) = image_name(hdus, idx)?
            {
                images.push(name);
            }
        }
        for (pos, claim) in claims.iter().enumerate() {
            let same = |name: &String| name.eq_ignore_ascii_case(&claim.image);
            if !images.iter().any(same) {
                return Err(FitsLoadError::from(FitsRefusal::ClaimsMissingImage {
                    extension,
                    hdu: claim.hdu,
                    image: claim.image.clone(),
                }));
            }
            if claims[..pos].iter().any(|earlier| same(&earlier.image)) {
                return Err(FitsLoadError::from(FitsRefusal::DuplicateClaim {
                    extension,
                    image: claim.image.clone(),
                }));
            }
        }
        Ok(Self { claims })
    }

    /// The extension for the image at HDU `image`; `None` when it has none.
    pub(super) fn for_image(
        &self,
        hdus: &[Hdu],
        image: usize,
    ) -> Result<Option<usize>, FitsLoadError> {
        let Some(name) = image_name(hdus, image)? else {
            return Ok(None);
        };
        Ok(self
            .claims
            .iter()
            .find(|claim| claim.image.eq_ignore_ascii_case(&name))
            .map(|claim| claim.hdu))
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
            if extension.names(&hdu.header)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// The name an extension refers to the image at HDU `idx` by: its `EXTNAME`, or [`PRIMARY`] for
/// an unnamed primary HDU; `None` for an unnamed extension, which none can name.
fn image_name(hdus: &[Hdu], idx: usize) -> Result<Option<String>, FitsLoadError> {
    let extname = metadata::read_text(&hdus[idx].header, "EXTNAME")?;
    Ok(match extname {
        Some(extname) => Some(extname),
        None => (idx == 0).then(|| PRIMARY.to_owned()),
    })
}
