//! [`ExtensionClaims`]: the Lumos image extensions of a FITS file that describe another of its
//! images, and the image each is for.

use std::path::Path;

use fits_well::io::Hdu;

use crate::io::error::{FitsRefusal, ImageError};
use crate::io::fits::hdu_list::HduList;
use crate::io::fits::lumos_extension::LumosExtension;
use crate::io::fits::metadata;

/// The keyword that names the image HDU an extension is for: its `EXTNAME`, or [`PRIMARY`].
pub(crate) const IMAGE_KEYWORD: &str = "LUMFOR";
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
    /// The `extension` HDUs of `file`, refused unless each names one image of the file that no
    /// other names.
    pub(crate) fn of(file: HduList<'_>, extension: LumosExtension) -> Result<Self, ImageError> {
        let HduList { path, hdus } = file;
        let mut images = Vec::new();
        let mut claims = Vec::new();
        for (index, hdu) in hdus.iter().enumerate() {
            let name = metadata::read_text(&hdu.header, "EXTNAME")
                .map_err(|source| ImageError::fits(path, source))?;
            if name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(extension.extname()))
            {
                let image = metadata::read_text(&hdu.header, IMAGE_KEYWORD)
                    .map_err(|source| ImageError::fits(path, source))?
                    .ok_or_else(|| {
                        ImageError::fits_unsupported(
                            path,
                            FitsRefusal::ClaimsNoImage {
                                extension,
                                hdu: index,
                            },
                        )
                    })?;
                claims.push(Claim { hdu: index, image });
            } else if hdu.is_image()
                && hdu.data_bytes > 0
                && !Self::describes_another(path, hdu)?
                && let Some(name) = image_name(file, index)?
            {
                images.push(name);
            }
        }
        for (position, claim) in claims.iter().enumerate() {
            let same = |name: &String| name.eq_ignore_ascii_case(&claim.image);
            if !images.iter().any(same) {
                return Err(ImageError::fits_unsupported(
                    path,
                    FitsRefusal::ClaimsMissingImage {
                        extension,
                        hdu: claim.hdu,
                        image: claim.image.clone(),
                    },
                ));
            }
            if claims[..position]
                .iter()
                .any(|earlier| same(&earlier.image))
            {
                return Err(ImageError::fits_unsupported(
                    path,
                    FitsRefusal::DuplicateClaim {
                        extension,
                        image: claim.image.clone(),
                    },
                ));
            }
        }
        Ok(Self { claims })
    }

    /// The extension for the image at HDU `image`; `None` when it has none.
    pub(crate) fn for_image(
        &self,
        file: HduList<'_>,
        image: usize,
    ) -> Result<Option<usize>, ImageError> {
        let Some(name) = image_name(file, image)? else {
            return Ok(None);
        };
        Ok(self
            .claims
            .iter()
            .find(|claim| claim.image.eq_ignore_ascii_case(&name))
            .map(|claim| claim.hdu))
    }

    /// Whether `hdu` is an extension that describes another image rather than being one.
    pub(crate) fn describes_another(path: &Path, hdu: &Hdu) -> Result<bool, ImageError> {
        Ok(metadata::read_text(&hdu.header, "EXTNAME")
            .map_err(|source| ImageError::fits(path, source))?
            .is_some_and(|extname| {
                LumosExtension::ALL
                    .iter()
                    .any(|describing| extname.eq_ignore_ascii_case(describing.extname()))
            }))
    }
}

/// The name an extension refers to the image at HDU `index` by: its `EXTNAME`, or [`PRIMARY`] for
/// an unnamed primary HDU; `None` for an unnamed extension, which none can name.
fn image_name(file: HduList<'_>, index: usize) -> Result<Option<String>, ImageError> {
    let extname = metadata::read_text(&file.hdus[index].header, "EXTNAME")
        .map_err(|source| ImageError::fits(file.path, source))?;
    Ok(match extname {
        Some(extname) => Some(extname),
        None => (index == 0).then(|| PRIMARY.to_owned()),
    })
}
