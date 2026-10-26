//! [`SessionThread`]: the one thread the editor lives on, and the edge where a panic in it ends
//! the session instead of the server.

use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use common::thread as threads;
use darkroom::internals::harness::error::HarnessError;
use darkroom::internals::harness::{Harness, SessionConfig};
use rmcp::model::{CallToolResult, ContentBlock};
use tokio::sync::oneshot;

/// Work for the editor's thread, answered with the tool's content or the error the agent reads.
type Job = Box<dyn FnOnce(&mut Session) -> Result<Vec<ContentBlock>, ToolError> + Send>;

/// The thread that owns the editor; tools hand it jobs in turn. The editor's types hold `Rc`s
/// and a GPU device, so they never leave the thread they are made on.
#[derive(Debug)]
pub(crate) struct SessionThread {
    // Dropped before `_thread`, which then joins: the closed channel ends the thread's loop.
    jobs: Sender<Request>,
    _thread: Joined,
}

/// The editor's thread, joined on drop: the process does not exit while the editor's GPU
/// device and worker are still being torn down, which crashes the driver.
#[derive(Debug)]
struct Joined(Option<JoinHandle<()>>);

struct Request {
    job: Job,
    reply: oneshot::Sender<CallToolResult>,
}

/// What the editor's thread holds between jobs.
#[derive(Debug)]
pub(crate) struct Session {
    /// What a new session is made with when `darkroom_open` names nothing else.
    pub(crate) defaults: SessionConfig,
    harness: Option<Harness>,
    /// Why the last session ended, when a panic ended it.
    ended: Option<String>,
}

/// A failed call, in words that tell the agent how to correct it.
#[derive(Debug)]
pub(crate) struct ToolError(pub(crate) String);

impl SessionThread {
    /// Start the editor's thread; its sessions are made with `defaults`.
    pub(crate) fn new(defaults: SessionConfig) -> Self {
        let (jobs, requests) = mpsc::channel();
        let panics: Arc<Mutex<Option<String>>> = Arc::default();
        let recorded = Arc::clone(&panics);
        let handle = threads::spawn("darkroom-mcp-session", move || {
            serve(&requests, defaults, &recorded);
        });
        let session_thread = handle.thread().id();
        // The payload alone has no location; the hook sees both, for this thread's panics only.
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if thread::current().id() == session_thread {
                let message = info.payload_as_str().unwrap_or("a panic with no message");
                let location = info
                    .location()
                    .map_or_else(String::new, |location| format!(" at {location}"));
                *panics.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(format!("{message}{location}"));
            }
            previous(info);
        }));
        Self {
            jobs,
            _thread: Joined(Some(handle)),
        }
    }

    /// Run `job` on the editor's thread, after the jobs before it.
    pub(crate) async fn run(
        &self,
        job: impl FnOnce(&mut Session) -> Result<Vec<ContentBlock>, ToolError> + Send + 'static,
    ) -> CallToolResult {
        let (reply, answer) = oneshot::channel();
        self.jobs
            .send(Request {
                job: Box::new(job),
                reply,
            })
            .expect("the editor's thread lives as long as the server");
        answer
            .await
            .expect("the editor's thread answers every job it takes")
    }
}

/// Take jobs until the server drops its end, each inside the panic edge.
fn serve(requests: &Receiver<Request>, defaults: SessionConfig, panics: &Mutex<Option<String>>) {
    let mut session = Session {
        defaults,
        harness: None,
        ended: None,
    };
    while let Ok(Request { job, reply }) = requests.recv() {
        let result = match panic::catch_unwind(AssertUnwindSafe(|| job(&mut session))) {
            Ok(Ok(content)) => CallToolResult::success(content),
            Ok(Err(ToolError(message))) => CallToolResult::error(vec![ContentBlock::text(message)]),
            Err(_) => {
                let panic = panics
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                    .unwrap_or_else(|| "a panic the hook did not see".to_string());
                session.end(panic.clone());
                CallToolResult::error(vec![ContentBlock::text(format!(
                    "the editor panicked: {panic}. The session is closed; report the panic, and call \
                     darkroom_open to start a new one."
                ))])
            }
        };
        // The server dropped the call: no one waits for its answer.
        reply.send(result).ok();
    }
}

impl Session {
    /// The open session, or the error that says why none is.
    pub(crate) fn harness(&mut self) -> Result<&mut Harness, ToolError> {
        match (&mut self.harness, &self.ended) {
            (Some(harness), _) => Ok(harness),
            (None, Some(panic)) => Err(ToolError(format!(
                "no project is open: the last session ended when the editor panicked: {panic}. Call \
                 darkroom_open to start a new one."
            ))),
            (None, None) => Err(ToolError(
                "no project is open; call darkroom_open first".to_string(),
            )),
        }
    }

    /// A new session of `config` on `project`, or on a new document for none, in place of any
    /// open one. A project that does not open leaves no session, rather than an empty document
    /// the agent did not ask for.
    pub(crate) fn start(
        &mut self,
        config: SessionConfig,
        project: Option<&Path>,
        in_place: bool,
    ) -> Result<&mut Harness, HarnessError> {
        // The old session first: its log layer is this thread's default until it drops.
        self.harness = None;
        self.ended = None;
        let mut harness = Harness::new(config);
        if let Some(project) = project {
            harness.open(project, in_place)?;
        }
        Ok(self.harness.insert(harness))
    }

    /// Drop the session a panic left, whose state no call can trust.
    fn end(&mut self, panic: String) {
        // Its drop may panic too, on the same broken state; the session is gone either way.
        let harness = self.harness.take();
        panic::catch_unwind(AssertUnwindSafe(move || drop(harness))).ok();
        self.ended = Some(panic);
    }
}

impl Drop for Joined {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            // A panic on the way out already reached the hook and stderr.
            handle.join().ok();
        }
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request").finish_non_exhaustive()
    }
}

impl From<HarnessError> for ToolError {
    fn from(error: HarnessError) -> Self {
        Self(error.to_string())
    }
}
