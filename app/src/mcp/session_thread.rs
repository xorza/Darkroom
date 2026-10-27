//! [`SessionThread`]: the one thread the editor lives on, and the edge where a panic in it ends
//! the session, which the server ends with, rather than unwinding through the server.

use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use common::thread as threads;
use darkroom::driver::error::DriverError;
use darkroom::driver::{Driver, OpenMode, SessionConfig};
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

/// What the editor's thread holds between jobs: the driver until the session ends, then why it
/// ended.
#[derive(Debug)]
pub(crate) struct Session {
    driver: Option<Driver>,
    ended: Option<Ended>,
    /// Told once, when the session ends, so the server ends with it.
    on_end: Option<oneshot::Sender<Ended>>,
}

/// Why a session ended.
#[derive(Clone, Debug)]
pub(crate) enum Ended {
    /// The editor panicked, with this message and location.
    Panic(String),
    /// An action asked the editor to quit, as File ▸ Quit or Ctrl+Q do.
    Quit,
}

/// A failed call, in words that tell the agent how to correct it.
#[derive(Debug)]
pub(crate) struct ToolError(pub(crate) String);

impl SessionThread {
    /// Start the editor's thread on the session `make` makes there, and the receiver told when
    /// that session ends; or the error that kept the session from starting.
    pub(crate) fn start(
        make: impl FnOnce() -> Result<Driver, DriverError> + Send + 'static,
    ) -> Result<(Self, oneshot::Receiver<Ended>), DriverError> {
        let (jobs, requests) = mpsc::channel();
        let (started, startup) = mpsc::channel();
        let (on_end, ended) = oneshot::channel();
        let panics: Arc<Mutex<Option<String>>> = Arc::default();
        let recorded = Arc::clone(&panics);
        let handle = threads::spawn("darkroom-mcp-session", move || {
            let driver = match make() {
                Ok(driver) => driver,
                Err(error) => {
                    started.send(Err(error)).ok();
                    return;
                }
            };
            started.send(Ok(())).ok();
            let session = Session {
                driver: Some(driver),
                ended: None,
                on_end: Some(on_end),
            };
            serve(&requests, session, &recorded);
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
        let thread = Self {
            jobs,
            _thread: Joined(Some(handle)),
        };
        // A thread that ends before it answers panicked, which the hook printed.
        match startup.recv() {
            Ok(Ok(())) => Ok((thread, ended)),
            Ok(Err(error)) => Err(error),
            Err(mpsc::RecvError) => panic!("the editor's thread panicked as the session started"),
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

/// An offscreen driver of `config` on `document`, or on a new document for none.
pub(crate) fn launch(
    config: SessionConfig,
    document: Option<&Path>,
) -> Result<Driver, DriverError> {
    let mut driver = Driver::new(config)?;
    if let Some(document) = document {
        driver.open(document, OpenMode::InPlace)?;
    }
    Ok(driver)
}

/// Take jobs until the server drops its end, each inside the panic edge.
fn serve(requests: &Receiver<Request>, mut session: Session, panics: &Mutex<Option<String>>) {
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
                    "the editor panicked: {panic}. The session is closed and the server ends with \
                     it; report the panic."
                ))])
            }
        };
        // The server dropped the call: no one waits for its answer.
        reply.send(result).ok();
    }
}

impl Session {
    /// The session's driver, or the error that says how the session ended.
    pub(crate) fn driver(&mut self) -> Result<&mut Driver, ToolError> {
        match (&mut self.driver, &self.ended) {
            (Some(driver), _) => Ok(driver),
            (None, Some(Ended::Panic(panic))) => Err(ToolError(format!(
                "the session ended when the editor panicked: {panic}; the server is ending"
            ))),
            (None, Some(Ended::Quit)) => Err(ToolError(
                "the session ended when an action quit the editor; the server is ending"
                    .to_string(),
            )),
            (None, None) => unreachable!("a session without a driver has ended"),
        }
    }

    /// Open `project` where `mode` says, or a new document for none, in place of the open one.
    /// A project that does not open leaves the open one as it was.
    pub(crate) fn open(
        &mut self,
        project: Option<&Path>,
        mode: OpenMode,
    ) -> Result<&mut Driver, ToolError> {
        let driver = self.driver()?;
        match project {
            Some(project) => driver.open(project, mode)?,
            None => driver.new_document()?,
        }
        Ok(driver)
    }

    /// End the session an action quit, as a window closes when its editor quits.
    pub(crate) fn quit(&mut self) {
        self.driver = None;
        self.ended(Ended::Quit);
    }

    /// End the session a panic left, whose state no call can trust.
    fn end(&mut self, panic: String) {
        // Its drop may panic too, on the same broken state; the session is gone either way.
        let driver = self.driver.take();
        panic::catch_unwind(AssertUnwindSafe(move || drop(driver))).ok();
        self.ended(Ended::Panic(panic));
    }

    fn ended(&mut self, ended: Ended) {
        if let Some(on_end) = self.on_end.take() {
            // The server already gone: no one waits for the end.
            on_end.send(ended.clone()).ok();
        }
        self.ended = Some(ended);
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

impl From<DriverError> for ToolError {
    fn from(error: DriverError) -> Self {
        Self(error.to_string())
    }
}
