//! [`FrameStep`]: what a decoded frame goes through before its checks and statistics.

use std::fmt::Debug;

use crate::calibration::error::SubtractionError;

/// A step each decoded frame of a run takes before its checks and statistics, given the frame's
/// index: the subtraction of a calibration master, for one. A frame through a step is not what its
/// source decodes to, so it is never committed to a kept cache.
pub(crate) trait FrameStep<I>: Sync + Debug {
    /// Why the step could not be taken on a frame.
    type Error;

    fn apply(&self, index: usize, image: &mut I) -> Result<(), Self::Error>;

    /// Whether the step changes a frame at all; a run whose step does not reads and writes the
    /// kept cache.
    fn changes_frames(&self) -> bool {
        true
    }
}

/// Frame files, and the step each takes as it decodes, given its index into `paths`.
#[derive(Debug)]
pub(crate) struct SteppedPaths<'a, P, S> {
    pub(crate) paths: &'a [P],
    pub(crate) step: &'a S,
}

/// Copied whatever its type parameters are, as it holds references alone: a derive would ask each
/// to be `Copy`.
impl<P, S> Clone for SteppedPaths<'_, P, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P, S> Copy for SteppedPaths<'_, P, S> {}

/// A step that leaves every frame as it decodes.
#[derive(Debug)]
pub(crate) struct NoStep;

impl<I> FrameStep<I> for NoStep {
    type Error = SubtractionError;

    fn apply(&self, _index: usize, _image: &mut I) -> Result<(), SubtractionError> {
        Ok(())
    }

    fn changes_frames(&self) -> bool {
        false
    }
}

/// A step a run takes or not, as it decides: `None` leaves every frame as it decodes.
impl<I, S: FrameStep<I>> FrameStep<I> for Option<S> {
    type Error = S::Error;

    fn apply(&self, index: usize, image: &mut I) -> Result<(), S::Error> {
        self.as_ref()
            .map_or(Ok(()), |step| step.apply(index, image))
    }

    fn changes_frames(&self) -> bool {
        self.as_ref().is_some_and(FrameStep::changes_frames)
    }
}

#[cfg(test)]
mod tests {
    use crate::calibration::error::SubtractionError;
    use crate::ingest::frame_step::{FrameStep, NoStep};

    /// Adds the frame's index, so a test sees which frames went through it.
    #[derive(Debug)]
    struct AddIndex;

    impl FrameStep<usize> for AddIndex {
        type Error = SubtractionError;

        fn apply(&self, index: usize, image: &mut usize) -> Result<(), SubtractionError> {
            *image += index;
            Ok(())
        }
    }

    /// A step taken applies and changes frames; one not taken, `None` or [`NoStep`], leaves the
    /// frame and changes none, so the run may keep its cache.
    #[test]
    fn a_step_not_taken_leaves_every_frame_as_it_decodes() {
        let cases: [(&dyn FrameStep<usize, Error = SubtractionError>, usize, bool); 3] = [
            (&Some(AddIndex), 10 + 3, true),
            (&None::<AddIndex>, 10, false),
            (&NoStep, 10, false),
        ];
        for (step, want, changes) in cases {
            let mut image = 10;
            step.apply(3, &mut image).unwrap();
            assert_eq!((image, step.changes_frames()), (want, changes), "{step:?}");
        }
    }
}
