//! [`PreviewConverter`]: the thread that turns published values into rasters.

use std::sync::mpsc::{self, Receiver, Sender, TryIter};
use std::thread;

use glam::UVec2;
use imaginarium::ColorFormat;
use lens::Image as LensImage;
use palantir::Image as Raster;
use scenarium::{DynamicValue, NodeId};

use crate::core::wake::Wake;
use crate::gui::state::preview_store::error::DrawablePreviewError;
use crate::gui::state::preview_store::{
    PREVIEW_TEXTURE_DIM, capped_target, native_size, rgba8_raster,
};

/// One background thread for the pixel work of a preview — reading an image
/// back to the CPU, downscaling it and converting it to RGBA8 — so that no
/// frame pays for it. A full-sensor frame takes hundreds of milliseconds of it;
/// the main thread only registers the finished raster.
///
/// Each finished job wakes the host, so a result lands on the next frame even
/// when nothing else would draw one. The thread ends when its last sender is
/// dropped: the store's own, and the clone each stored image keeps for its
/// full-resolution ask.
#[derive(Debug)]
pub(crate) struct PreviewConverter {
    jobs: Jobs,
    results: Receiver<Converted>,
}

/// Where jobs are queued: the store's, and a clone in each stored image.
#[derive(Debug, Clone)]
pub(super) struct Jobs(Sender<Job>);

impl Jobs {
    /// Queue `job`. The thread only stops once every sender is gone, so a
    /// refused send means it panicked — a broken invariant, not a condition
    /// to carry on from.
    pub(super) fn send(&self, job: Job) {
        self.0
            .send(job)
            .expect("the preview converter thread panicked");
    }
}

/// Which publish of a node a job converts, so a result can tell whether it is
/// still the newest. Minted by the store, never reused.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Ticket(u64);

impl Ticket {
    pub(super) const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// One conversion, and the publish it belongs to.
#[derive(Debug)]
pub(super) struct Job {
    pub(super) node_id: NodeId,
    pub(super) ticket: Ticket,
    pub(super) work: Work,
}

#[derive(Debug)]
pub(super) enum Work {
    /// A freshly published value: an image's thumbnail, or a non-image's text.
    Thumbnail(DynamicValue),
    /// The full-resolution raster of an image whose thumbnail is resident,
    /// at `texels` — its native size capped to the device's texture limit.
    Full { source: DynamicValue, texels: UVec2 },
}

/// A finished [`Job`].
#[derive(Debug)]
pub(crate) struct Converted {
    pub(super) node_id: NodeId,
    pub(super) ticket: Ticket,
    pub(super) outcome: Outcome,
}

#[derive(Debug)]
pub(super) enum Outcome {
    /// What a freshly published value shows on its card.
    Card(Card),
    /// A viewer's full-resolution raster.
    Full(Raster),
}

/// What a published value shows on its card.
#[derive(Debug, Clone)]
pub(crate) enum Card {
    /// A non-image value, formatted.
    Text(String),
    /// An image: its thumbnail, and the source a viewer's full raster is
    /// converted from later.
    Image {
        thumbnail: Thumbnail,
        source: DynamicValue,
    },
    /// An image no texture can hold.
    Failed(DrawablePreviewError),
}

/// A preview card's raster, and the source figures it was cut from.
#[derive(Debug, Clone)]
pub(crate) struct Thumbnail {
    pub(super) raster: Raster,
    pub(super) native_size: UVec2,
    pub(super) native_format: ColorFormat,
}

impl PreviewConverter {
    pub(super) fn new(wake: Wake) -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (outbox, results) = mpsc::channel();
        thread::Builder::new()
            .name("darkroom-preview".to_owned())
            .spawn(move || {
                for job in inbox {
                    // The store is gone, and with it everyone who would read
                    // the result.
                    if outbox.send(job.run()).is_err() {
                        return;
                    }
                    wake();
                }
            })
            .expect("the OS starts the preview converter thread");
        Self {
            jobs: Jobs(jobs),
            results,
        }
    }

    /// Where jobs are queued — cloned into each image, which queues its own
    /// full-resolution job.
    pub(super) const fn jobs(&self) -> &Jobs {
        &self.jobs
    }

    /// Every result finished since the last call, without waiting.
    pub(super) fn ready(&self) -> TryIter<'_, Converted> {
        self.results.try_iter()
    }
}

impl Job {
    fn run(self) -> Converted {
        let Self {
            node_id,
            ticket,
            work,
        } = self;
        let outcome = match work {
            Work::Thumbnail(value) => Outcome::Card(card(value)),
            Work::Full { source, texels } => {
                let image = source
                    .as_custom::<LensImage>()
                    .expect("a full raster is only asked of an image value");
                Outcome::Full(rgba8_raster(&image.interleaved(), texels))
            }
        };
        Converted {
            node_id,
            ticket,
            outcome,
        }
    }
}

/// What a published `value` shows on its card. The source moves into the
/// card only after the pixels are read, since the image borrows it.
fn card(value: DynamicValue) -> Card {
    let thumbnail = {
        let Some(image) = value.as_custom::<LensImage>() else {
            return Card::Text(value.to_string());
        };
        let cpu = image.interleaved();
        let desc = cpu.desc();
        match native_size(desc) {
            Ok(native_size) => Thumbnail {
                raster: rgba8_raster(&cpu, capped_target(native_size, Some(PREVIEW_TEXTURE_DIM))),
                native_size,
                native_format: desc.color_format,
            },
            Err(error) => return Card::Failed(error),
        }
    };
    Card::Image {
        thumbnail,
        source: value,
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use scenarium::DynamicValue;

    use crate::gui::state::preview_store::preview_converter::{Card, card};
    #[cfg(test)]
    use crate::gui::state::preview_store::preview_converter::{Converted, PreviewConverter};

    #[cfg(test)]
    impl PreviewConverter {
        /// The next result, waiting for it — what a test or a bench does
        /// where production waits for the next frame.
        pub(crate) fn wait(&self) -> Converted {
            self.results
                .recv()
                .expect("the preview converter thread panicked")
        }
    }

    /// `value`'s card, converted on the calling thread — for a bench that
    /// lands one conversion every frame and times the landing alone.
    pub(crate) fn convert_here(value: DynamicValue) -> Card {
        card(value)
    }
}
