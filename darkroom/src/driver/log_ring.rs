//! [`LogRing`]: the warnings and errors the editor logs, kept for the driver to report.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::filter::{Filtered, LevelFilter};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// Lines kept; older ones go first.
const CAPACITY: usize = 200;

/// The last warnings and errors logged, as lines, `LEVEL target: message`, which its
/// filtered layer feeds.
#[derive(Clone, Debug, Default)]
pub struct LogRing {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl LogRing {
    /// The layer that fills the ring with warnings and errors. Filtered per layer: `RUST_LOG`
    /// hides no warning from the ring, and an event no layer takes is never built.
    pub(crate) fn layer<S>(&self) -> Filtered<RingLayer, LevelFilter, S>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        RingLayer(self.clone()).with_filter(LevelFilter::WARN)
    }

    /// The lines logged since the last call, oldest first.
    pub(crate) fn drain(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }
}

/// The ring as a tracing layer, which only [`LogRing::layer`] hands out, so the ring holds
/// warnings and errors alone whoever installs it.
#[derive(Debug)]
pub(crate) struct RingLayer(LogRing);

impl<S: Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut message = Message::default();
        event.record(&mut message);
        let line = format!("{} {}: {}", metadata.level(), metadata.target(), message.0);
        let mut lines = self.0.lines.lock().unwrap_or_else(PoisonError::into_inner);
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
    }
}

/// An event's `message` field, then its other fields as `name=value`.
#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        use std::fmt::Write as _;

        if !self.0.is_empty() {
            self.0.push(' ');
        }
        if field.name() == "message" {
            write!(self.0, "{value:?}").ok();
        } else {
            write!(self.0, "{}={value:?}", field.name()).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing::subscriber;
    use tracing_subscriber::layer::SubscriberExt as _;

    use crate::driver::log_ring::{CAPACITY, LogRing};

    /// Warnings and errors are kept and info is not; a drain empties the ring; past the capacity
    /// the oldest line goes first.
    #[test]
    fn the_ring_keeps_the_last_warnings_and_errors() {
        let ring = LogRing::default();
        let _guard = subscriber::set_default(tracing_subscriber::registry().with(ring.layer()));
        tracing::info!("skipped");
        tracing::warn!(port = 2, "a warning");
        tracing::error!("an error");
        let target = module_path!();
        assert_eq!(
            ring.drain(),
            [
                format!("WARN {target}: a warning port=2"),
                format!("ERROR {target}: an error"),
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
