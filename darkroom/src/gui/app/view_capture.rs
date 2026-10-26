//! [`ViewCapture`]: an image of the window the user asked for, from the command to the frame its
//! screenshot lands in.

use std::io::{BufWriter, Write as _};
use std::path::PathBuf;

use common::fs::{self, PublicationMode};
use image::codecs::png::PngEncoder;
use image::{ImageError, RgbaImage};
use palantir::{Clipboard, Image, Screenshot, Ui};

use crate::gui::app::error::ViewCaptureError;

/// Where a captured image goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CaptureTarget {
    Clipboard,
    /// A PNG file, replaced whole or left as it was.
    Png(PathBuf),
}

/// A capture waiting on its screenshot: asked of the frame once, then delivered when the image
/// lands. A frame's image comes a frame or more after the ask, so the target waits here.
#[derive(Debug)]
pub(crate) struct ViewCapture {
    target: CaptureTarget,
    asked: bool,
}

impl ViewCapture {
    pub(crate) const fn new(target: CaptureTarget) -> Self {
        Self {
            target,
            asked: false,
        }
    }

    /// Ask `ui`'s frame for an image of the whole window, once.
    pub(crate) fn ask(&mut self, ui: &mut Ui) {
        if !self.asked {
            ui.request_screenshot(None);
            self.asked = true;
        }
    }

    /// Whether the ask went out, so an image that lands now is this capture's.
    pub(crate) const fn asked(&self) -> bool {
        self.asked
    }

    /// Put `shot` where the target says.
    pub(crate) fn deliver(
        self,
        shot: &Screenshot,
        clipboard: &Clipboard,
    ) -> Result<(), ViewCaptureError> {
        // The window's size at paint time decides the region, and a window of no pixels then
        // gives none.
        let image = shot.image().ok_or(ViewCaptureError::Empty)?;
        match self.target {
            CaptureTarget::Clipboard => Ok(clipboard.set_image(image)?),
            CaptureTarget::Png(path) => fs::publish(&path, PublicationMode::Durable, |file| {
                let mut out = BufWriter::new(file);
                rgba_image(image).write_with_encoder(PngEncoder::new(&mut out))?;
                out.flush()?;
                Ok::<_, ImageError>(())
            })
            .map_err(|source| ViewCaptureError::Save { path, source }),
        }
    }
}

/// `image`'s texels as the `image` crate holds them, for its codecs and filters.
pub(crate) fn rgba_image(image: &Image) -> RgbaImage {
    let size = image.size();
    let bytes = image
        .texels()
        .iter()
        .flat_map(|texel| [texel.r, texel.g, texel.b, texel.a])
        .collect();
    RgbaImage::from_raw(size.x, size.y, bytes).expect("four bytes per texel")
}
