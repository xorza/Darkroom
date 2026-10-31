//! [`ViewCapture`]: an image of the window the user asked for, from the command to the frame its
//! screenshot lands in.

use std::io::{BufWriter, Write};
use std::path::PathBuf;

use common::fs::{self, PublicationMode};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageError};
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
                write_png(image, &mut out)?;
                out.flush()?;
                Ok::<_, ImageError>(())
            })
            .map_err(|source| ViewCaptureError::Save { path, source }),
        }
    }
}

/// Encode `image` as a PNG into `out`, straight from its bytes.
pub(crate) fn write_png(image: &Image, out: impl Write) -> Result<(), ImageError> {
    let size = image.size();
    PngEncoder::new(out).write_image(image.as_srgba8(), size.x, size.y, ExtendedColorType::Rgba8)
}
