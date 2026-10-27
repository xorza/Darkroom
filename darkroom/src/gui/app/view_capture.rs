//! [`ViewCapture`]: an image of the window the user asked for, from the command to the frame its
//! screenshot lands in.

use std::io::{BufWriter, Write as _};
use std::path::PathBuf;

use common::fs::{self, PublicationMode};
use image::codecs::png::PngEncoder;
use image::{ImageError, RgbaImage};
use palantir::{Clipboard, Image, Ui};

use crate::gui::app::error::ViewCaptureError;
use crate::gui::app::screenshot_ask::{Landed, ScreenshotAsk};

/// Where a captured image goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CaptureTarget {
    Clipboard,
    /// A PNG file, replaced whole or left as it was.
    Png(PathBuf),
}

/// A capture waiting on its screenshot: asked of the frame once, then delivered when the image
/// of its ask lands. A frame's image comes a frame or more after the ask, so the target waits
/// here.
#[derive(Debug)]
pub(crate) struct ViewCapture {
    target: CaptureTarget,
    asked: ScreenshotAsk,
}

impl ViewCapture {
    pub(crate) const fn new(target: CaptureTarget) -> Self {
        Self {
            target,
            asked: ScreenshotAsk::Unasked,
        }
    }

    /// Ask `ui`'s frame for an image of the whole window, once.
    pub(crate) fn ask(&mut self, ui: &mut Ui) {
        self.asked.ask(ui, None);
    }

    /// What this capture's ask got, once it got it.
    pub(crate) fn landed<'ui>(&self, ui: &'ui Ui) -> Option<Landed<'ui>> {
        self.asked.landed(ui)
    }

    /// Put what landed where the target says.
    pub(crate) fn deliver(
        self,
        landed: Landed<'_>,
        clipboard: &Clipboard,
    ) -> Result<(), ViewCaptureError> {
        let Landed::Shot(shot) = landed else {
            return Err(ViewCaptureError::Empty);
        };
        let image = shot.image();
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
    // Through whole texels, then flattened: an exact-size map, where a flat map's unknown length
    // regrows the buffer as it fills.
    let bytes = image
        .texels()
        .iter()
        .map(|texel| [texel.r, texel.g, texel.b, texel.a])
        .collect::<Vec<_>>()
        .into_flattened();
    RgbaImage::from_raw(size.x, size.y, bytes).expect("four bytes per texel")
}
