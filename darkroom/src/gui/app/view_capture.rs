//! [`ViewCapture`]: an image of the window the user asked for, from the command to the frame its
//! screenshot lands in.

use std::io::BufWriter;
use std::path::PathBuf;

use common::fs::{self, PublicationMode};
use palantir::{Clipboard, Ui};

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
    pub(crate) fn landed<'c>(&self, ui: &'c Ui) -> Option<Landed<'c>> {
        self.asked.landed(ui)
    }

    /// Put what landed where the target says.
    pub(crate) fn deliver(
        self,
        landed: Landed<'_>,
        clipboard: &Clipboard,
    ) -> Result<(), ViewCaptureError> {
        let shot = match landed {
            Landed::Shot(shot) => shot,
            Landed::Refused(refusal) => return Err(ViewCaptureError::Refused(refusal)),
        };
        let image = shot.image();
        match self.target {
            CaptureTarget::Clipboard => {
                clipboard.set_image(image);
                Ok(())
            }
            CaptureTarget::Png(path) => fs::publish(&path, PublicationMode::Durable, |file| {
                image.write_png(BufWriter::new(file))
            })
            .map_err(|source| ViewCaptureError::Save { path, source }),
        }
    }
}
