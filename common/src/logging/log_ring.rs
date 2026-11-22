//! [`LogRing`]: the last warnings and errors logged, kept for a caller to report.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::MutexExt;
use crate::logging::logger::Logger;

/// Lines kept; older ones go first.
const CAPACITY: usize = 200;

/// The last warnings and errors logged, as lines, `LEVEL target: message fields`, which a
/// [`Logger`] given it fills whatever its filter, so `RUST_LOG` hides no warning from the ring.
#[derive(Clone, Debug, Default)]
pub struct LogRing {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl LogRing {
    /// A logger that fills this ring alone and writes no line, for a scope's
    /// `tracing::subscriber::set_default`.
    pub fn logger(&self) -> Logger {
        Logger::ring_only(self.clone())
    }

    /// The lines logged since the last call, oldest first.
    pub fn drain(&self) -> Vec<String> {
        self.lines.lock_unpoisoned().drain(..).collect()
    }

    pub(super) fn push(&self, line: String) {
        let mut lines = self.lines.lock_unpoisoned();
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fmt;

    use tracing::subscriber;

    use crate::logging::log_ring::{CAPACITY, LogRing};

    #[derive(Debug)]
    struct Outer(Inner);

    #[derive(Debug)]
    struct Inner;

    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("the save failed")
        }
    }

    impl Error for Outer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    impl fmt::Display for Inner {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("the disk is full")
        }
    }

    impl Error for Inner {}

    /// Warnings and errors are kept and info is not; the message comes first, a string field is
    /// quoted and an error field reads as its chain; a drain empties the ring; past the capacity
    /// the oldest line goes first.
    #[test]
    fn the_ring_keeps_the_last_warnings_and_errors() {
        let ring = LogRing::default();
        let _guard = subscriber::set_default(ring.logger());
        tracing::info!("skipped");
        tracing::warn!(port = 2, name = "Blur", "a warning");
        let error: &(dyn Error + 'static) = &Outer(Inner);
        tracing::error!(error, "an error");
        let target = module_path!();
        assert_eq!(
            ring.drain(),
            [
                format!("WARN {target}: a warning port=2 name=\"Blur\""),
                format!("ERROR {target}: an error error=the save failed: the disk is full"),
            ]
        );
        assert!(ring.drain().is_empty());
        for i in 0..=CAPACITY {
            tracing::warn!("{i}");
        }
        let lines = ring.drain();
        assert_eq!(lines.len(), CAPACITY);
        assert_eq!(lines[0], format!("WARN {target}: 1"));
    }
}
