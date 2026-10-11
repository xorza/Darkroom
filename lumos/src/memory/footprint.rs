//! [`Footprint`]: what a step on a frame's way holds.

/// What a step on a frame's way holds: what it keeps of the frame once done, and the most it
/// holds on the way, what it keeps included. Steps chain: each starts from what the one before
/// kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Footprint {
    pub(crate) kept: usize,
    pub(crate) peak: usize,
}

impl Footprint {
    /// A frame of `bytes`, which nothing has worked on yet.
    pub(crate) const fn kept(bytes: usize) -> Self {
        Self {
            kept: bytes,
            peak: bytes,
        }
    }

    /// This, then a step that holds `bytes` beside what this keeps and leaves it as it was.
    pub(crate) const fn beside(self, bytes: usize) -> Self {
        let during = self.kept.saturating_add(bytes);
        Self {
            kept: self.kept,
            peak: if during > self.peak {
                during
            } else {
                self.peak
            },
        }
    }

    /// This, then `next`, a step that starts from what this keeps: its peak counts that too.
    pub(crate) const fn then(self, next: Self) -> Self {
        Self {
            kept: next.kept,
            peak: if next.peak > self.peak {
                next.peak
            } else {
                self.peak
            },
        }
    }

    /// What the way holds beyond what it keeps at its end.
    pub(crate) const fn transient(self) -> usize {
        self.peak.saturating_sub(self.kept)
    }
}

#[cfg(test)]
mod tests {
    use crate::memory::footprint::Footprint;

    /// A 100 B frame: decoded beside 40 B of its decoder's, 140; 30 B measured beside it leave the
    /// peak there; 70 B beside it raise it to 170. Demosaiced into 300 B at a peak of 450, then
    /// 200 B beside the 300: 500, of which 200 are transient.
    #[test]
    fn steps_chain_from_what_the_step_before_kept() {
        let decoded = Footprint {
            kept: 100,
            peak: 140,
        };
        assert_eq!(decoded.beside(30), decoded);
        let measured = decoded.beside(70);
        assert_eq!(
            measured,
            Footprint {
                kept: 100,
                peak: 170
            }
        );
        let demosaiced = measured.then(Footprint {
            kept: 300,
            peak: 450,
        });
        assert_eq!(
            demosaiced,
            Footprint {
                kept: 300,
                peak: 450
            }
        );
        let detected = demosaiced.beside(200);
        assert_eq!(
            detected,
            Footprint {
                kept: 300,
                peak: 500
            }
        );
        assert_eq!(detected.transient(), 200);
        assert_eq!(Footprint::kept(100).transient(), 0);
    }
}
