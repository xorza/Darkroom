//! [`Jobs`]: the preview converter's queue, which holds at most one waiting card conversion a
//! node, so a node that publishes faster than its values convert holds two values, the one
//! converting and the newest.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use crate::gui::state::preview_store::preview_converter::{Job, Work};

/// Where jobs are queued: the store's, and a clone in each stored image. The converter thread's
/// [`Inbox`] ends once the last is dropped.
pub(super) struct Jobs(Arc<Shared>);

/// The converter thread's end of the queue.
pub(super) struct Inbox(Arc<Shared>);

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// The [`Jobs`] alive, whose last drop ends the [`Inbox`].
    senders: usize,
    /// Whether the [`Inbox`] is alive: it dies only with the thread, which panicked then.
    inbox: bool,
}

/// A queue's two ends.
pub(super) fn queue() -> (Jobs, Inbox) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(Queue {
            senders: 1,
            inbox: true,
            ..Queue::default()
        }),
        ready: Condvar::new(),
    });
    (Jobs(Arc::clone(&shared)), Inbox(shared))
}

impl Jobs {
    /// Queue `job`. A card conversion replaces the one of its node still waiting, where it waits:
    /// its node keeps its place, and the value it replaces is dropped unconverted.
    ///
    /// # Panics
    ///
    /// When the converter thread panicked: a broken invariant, not a condition to carry on from.
    pub(super) fn send(&self, job: Job) {
        let mut queue = self.0.lock();
        assert!(queue.inbox, "the preview converter thread panicked");
        let waiting = if matches!(job.work, Work::Thumbnail(_)) {
            queue.jobs.iter_mut().find(|waiting| {
                waiting.node_id == job.node_id && matches!(waiting.work, Work::Thumbnail(_))
            })
        } else {
            None
        };
        match waiting {
            Some(waiting) => *waiting = job,
            None => queue.jobs.push_back(job),
        }
        self.0.ready.notify_one();
    }
}

impl Clone for Jobs {
    fn clone(&self) -> Self {
        self.0.lock().senders += 1;
        Self(Arc::clone(&self.0))
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        let mut queue = self.0.lock();
        queue.senders -= 1;
        if queue.senders == 0 {
            self.0.ready.notify_one();
        }
    }
}

impl Iterator for Inbox {
    type Item = Job;

    /// The next job, waiting for one; `None` once every [`Jobs`] is dropped and none waits.
    fn next(&mut self) -> Option<Job> {
        let mut queue = self.0.lock();
        loop {
            if let Some(job) = queue.jobs.pop_front() {
                return Some(job);
            }
            if queue.senders == 0 {
                return None;
            }
            queue = self
                .0
                .ready
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        self.0.lock().inbox = false;
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for Jobs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Jobs")
            .field("waiting", &self.0.lock().jobs.len())
            .finish()
    }
}

impl fmt::Debug for Inbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inbox")
            .field("waiting", &self.0.lock().jobs.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use glam::UVec2;
    use scenarium::{DynamicValue, NodeId};

    use crate::gui::state::preview_store::job_queue::queue;
    use crate::gui::state::preview_store::preview_converter::{Job, Ticket, Work};

    fn job(node_id: NodeId, ticket: Ticket, work: Work) -> Job {
        Job {
            node_id,
            ticket,
            work,
        }
    }

    /// A node's second card conversion replaces its first where it waits, and another node's and
    /// a full raster's queue behind it; the inbox hands them out in that order, and ends once the
    /// last sender is dropped and nothing waits.
    #[test]
    fn a_newer_card_replaces_the_one_waiting_and_the_inbox_ends_with_its_senders() {
        let (a, b) = (NodeId::unique(), NodeId::unique());
        let tickets: Vec<Ticket> = (0..4)
            .scan(Ticket::default(), |ticket, _| {
                *ticket = ticket.next();
                Some(*ticket)
            })
            .collect();
        let thumbnail = |value: i64| Work::Thumbnail(DynamicValue::from(value));
        let (jobs, inbox) = queue();
        let image = jobs.clone();
        jobs.send(job(a, tickets[0], thumbnail(1)));
        jobs.send(job(b, tickets[1], thumbnail(2)));
        image.send(job(
            a,
            tickets[2],
            Work::Full {
                source: DynamicValue::from(3_i64),
                texels: UVec2::ONE,
            },
        ));
        jobs.send(job(a, tickets[3], thumbnail(4)));
        drop((jobs, image));

        let order: Vec<(NodeId, Ticket, String)> = inbox
            .map(|job| {
                let work = match job.work {
                    Work::Thumbnail(value) => format!("card {value}"),
                    Work::Full { source, .. } => format!("full {source}"),
                };
                (job.node_id, job.ticket, work)
            })
            .collect();
        assert_eq!(
            order,
            [
                (a, tickets[3], "card 4".to_string()),
                (b, tickets[1], "card 2".to_string()),
                (a, tickets[2], "full 3".to_string()),
            ]
        );
    }
}
