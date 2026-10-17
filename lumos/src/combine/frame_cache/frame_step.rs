//! [`FrameStep`]: what a decoded frame goes through before its checks and statistics.

use std::convert::Infallible;
use std::fmt::Debug;

/// A step each decoded frame of a run takes before its checks and statistics, given the frame's
/// index: the subtraction of a calibration master, for one. A frame through a step is not what its
/// source decodes to, so its decode is what a kept cache commits, never the frame.
pub(crate) trait FrameStep<I>: Sync + Debug {
    /// Why the step could not be taken on a frame.
    type Error;

    fn apply(&self, index: usize, image: &mut I) -> Result<(), Self::Error>;

    /// Whether the step changes a frame at all; a run whose step does not takes each frame as the
    /// planes the decode cache keeps for it.
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

/// The step no run takes: a run with no step steps by `None::<Infallible>`.
impl<I> FrameStep<I> for Infallible {
    type Error = Self;

    fn apply(&self, _index: usize, _image: &mut I) -> Result<(), Self> {
        match *self {}
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
    use std::convert::Infallible;

    use crate::combine::frame_cache::frame_step::FrameStep;

    /// Adds the frame's index, so a test sees which frames went through it.
    #[derive(Debug)]
    struct AddIndex;

    impl FrameStep<usize> for AddIndex {
        type Error = Infallible;

        fn apply(&self, index: usize, image: &mut usize) -> Result<(), Infallible> {
            *image += index;
            Ok(())
        }
    }

    /// A step taken applies and changes frames; one not taken, `None` of any step, leaves the
    /// frame and changes none, so the run may keep its cache.
    #[test]
    fn a_step_not_taken_leaves_every_frame_as_it_decodes() {
        let cases: [(&dyn FrameStep<usize, Error = Infallible>, usize, bool); 3] = [
            (&Some(AddIndex), 10 + 3, true),
            (&None::<AddIndex>, 10, false),
            (&None::<Infallible>, 10, false),
        ];
        for (step, want, changes) in cases {
            let mut image = 10;
            step.apply(3, &mut image).unwrap();
            assert_eq!((image, step.changes_frames()), (want, changes), "{step:?}");
        }
    }
}
