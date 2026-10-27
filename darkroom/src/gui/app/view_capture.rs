//! [`ViewCapture`]: an image of the window the user asked for, from the command to the frame its
//! screenshot lands in.

use std::io::{BufWriter, Write as _};
use std::path::PathBuf;

use common::fs::{self, PublicationMode};
use image::codecs::png::PngEncoder;
use image::{ImageError, RgbaImage};
use palantir::{Clipboard, Image, Screenshot, ScreenshotId, Ui};

use crate::gui::app::error::ViewCaptureError;

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
    /// The ask, once it went out; a replayed pass of the asking frame asks again and gets the
    /// same one.
    asked: Option<ScreenshotId>,
}

impl ViewCapture {
    pub(crate) const fn new(target: CaptureTarget) -> Self {
        Self {
            target,
            asked: None,
        }
    }

    /// Ask `ui`'s frame for an image of the whole window, once.
    pub(crate) fn ask(&mut self, ui: &mut Ui) {
        if self.asked.is_none() {
            self.asked = Some(ui.request_screenshot(None));
        }
    }

    /// The image this capture asked for, in the frame `ui` it lands in.
    pub(crate) fn landed<'ui>(&self, ui: &'ui Ui) -> Option<&'ui Screenshot> {
        ui.screenshot_taken(self.asked?)
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
