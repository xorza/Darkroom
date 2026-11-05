//! [`PreviewConverter`]: the thread that turns published values into rasters.

use std::iter;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use common::thread;

use glam::UVec2;
use imaginarium::ColorFormat;
use lens::Image;
use palantir::Image as Raster;
use scenarium::{DynamicValue, NodeId};

use crate::core::wake::Wake;
use crate::gui::state::preview_store::error::DrawablePreviewError;
use crate::gui::state::preview_store::job_queue::{self, Jobs};
use crate::gui::state::preview_store::{
    PREVIEW_TEXTURE_DIM, capped_target, native_size, rgba8_raster,
};

/// One background thread for the pixel work of a preview — reading an image
/// back to the CPU, downscaling it and converting it to RGBA8 — so that no
/// frame pays for it. A full-sensor frame takes hundreds of milliseconds of it;
/// the main thread only registers the finished raster.
///
/// Each finished job wakes the host, so a result lands on the next frame even
/// when nothing else would draw one. The thread ends when the store drops its
/// [`Jobs`].
#[derive(Debug)]
pub(crate) struct PreviewConverter {
    jobs: Jobs,
    results: Receiver<Converted>,
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
        let (jobs, inbox) = job_queue::queue();
        let (outbox, results) = mpsc::channel();
        thread::spawn("darkroom-preview", move || {
            for job in inbox {
                // The store is gone, and with it everyone who would read
                // the result.
                if outbox.send(job.run()).is_err() {
                    return;
                }
                wake();
            }
        });
        Self { jobs, results }
    }

    /// Where the store queues jobs.
    pub(super) const fn jobs(&self) -> &Jobs {
        &self.jobs
    }

    /// Every result finished since the last call, without waiting.
    ///
    /// # Panics
    ///
    /// When the converter thread panicked, after the results it sent before: while the store
    /// holds its [`Jobs`], the thread ends only so.
    pub(super) fn ready(&self) -> impl Iterator<Item = Converted> + '_ {
        iter::from_fn(|| match self.results.try_recv() {
            Ok(converted) => Some(converted),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => panic!("the preview converter thread panicked"),
        })
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
                    .as_custom::<Image>()
                    .expect("a full raster is only asked of an image value");
                Outcome::Full(rgba8_raster(image, texels))
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
        let Some(image) = value.as_custom::<Image>() else {
            return Card::Text(value.to_string());
        };
        let desc = image.desc();
        match native_size(desc) {
            Ok(native_size) => Thumbnail {
                raster: rgba8_raster(image, capped_target(native_size, Some(PREVIEW_TEXTURE_DIM))),
                native_size,
                native_format: desc.color_format(),
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use scenarium::NodeId;

    use crate::gui::state::preview_store::job_queue;
    use crate::gui::state::preview_store::preview_converter::{
        Card, Converted, Outcome, PreviewConverter, Ticket,
    };

    /// A converter whose thread is gone hands out the result the thread sent first, then panics:
    /// a dead thread cannot leave its previews converting with no report.
    #[test]
    #[should_panic(expected = "the preview converter thread panicked")]
    fn a_dead_converter_thread_panics_after_its_results() {
        let (jobs, inbox) = job_queue::queue();
        drop(inbox);
        let (outbox, results) = mpsc::channel();
        let node_id = NodeId::unique();
        outbox
            .send(Converted {
                node_id,
                ticket: Ticket::default(),
                outcome: Outcome::Card(Card::Text("7".to_owned())),
            })
            .unwrap();
        drop(outbox);
        let converter = PreviewConverter { jobs, results };

        let mut ready = converter.ready();
        let first = ready
            .next()
            .expect("the result sent before the thread died");
        assert_eq!(first.node_id, node_id);
        let _ = ready.next();
    }
}
