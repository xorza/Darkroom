//! [`LightSource`]: where a registered stack's lights come from.

use crate::frame::frame_info::{DecoderBytes, FrameInfo};
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_run::IngestRun;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::light_files::LightFiles;
use crate::pipeline::light_shape::LightShape;

/// Where a registered stack's lights come from.
#[derive(Debug)]
pub(super) enum LightSource<'a> {
    /// Frames the caller decoded and handed over. They were in memory before the run read the
    /// machine, so the plan charges them only what the run adds, and they stay resident whatever
    /// the tier: spilling them would be a write and a read-back for nothing.
    Held(Vec<LinearImage>),
    /// Lights read from files, each made ready on a worker. Boxed: the files' readers and
    /// preparation are far larger than the held frames' `Vec`, by a margin that differs per target.
    Files(Box<LightFiles<'a>>),
}

impl LightSource<'_> {
    pub(super) const fn len(&self) -> usize {
        match self {
            Self::Held(frames) => frames.len(),
            Self::Files(files) => files.len(),
        }
    }

    /// The run's shape, from the first held frame or the first file's header: no light is decoded
    /// before the tier is chosen.
    pub(super) fn shape(&self, run: &IngestRun) -> Result<LightShape, AlignStackError> {
        match self {
            // They were in memory before the run read the machine: the plan charges them as held.
            Self::Held(frames) => Ok(LightShape::held(
                FrameInfo::of_decoded(&frames[0], DecoderBytes::default()),
                frames.len(),
                run.budget.workers(),
            )),
            Self::Files(files) => files.shape(run),
        }
    }
}
