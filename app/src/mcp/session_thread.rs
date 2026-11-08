//! [`SessionThread`]: the one thread the editor lives on, and the edge where a panic in it ends
//! the session, which the server ends with, rather than unwinding through the server.

use common::MutexExt;
use std::collections::HashMap;
use std::fmt;
use std::mem;
use std::panic::{self, AssertUnwindSafe, PanicHookInfo};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle, ThreadId};

use common::{ErrorChain, thread as threads};
use darkroom::driver::error::DriverError;
use darkroom::driver::log_ring::LogRing;
use darkroom::driver::{Driver, OpenMode, SessionConfig};
use rmcp::model::{CallToolResult, ContentBlock};
use tokio::sync::{mpsc, oneshot};

/// The last panic of each editor thread, by thread, which the panic hook writes. The hook is the
/// process's own, so its record is a global too: installed once, in front of the hook before it.
static PANICS: OnceLock<Mutex<HashMap<ThreadId, Option<String>>>> = OnceLock::new();

/// The jobs that wait for the editor's thread. Jobs run one at a time and each call waits for its
/// answer, so one waiting job keeps the thread busy; a call past it waits in its send. The queue
/// is a bounded `tokio::sync::mpsc` channel, which its docs give as the bridge between sync and
/// async code.
const WAITING_JOBS: usize = 1;

/// Work for the editor's thread, answered with the tool's content or the error the agent reads.
type Job = Box<dyn FnOnce(&mut Session) -> Result<Vec<ContentBlock>, ToolError> + Send>;

/// The thread that owns the editor; tools hand it jobs in turn. The editor's types hold `Rc`s
/// and a GPU device, so they never leave the thread they are made on.
#[derive(Debug)]
pub(crate) struct SessionThread {
    // Dropped before `_thread`, which then joins: the closed channel ends the thread's loop.
    jobs: mpsc::Sender<Request>,
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
pub(crate) enum Session {
    /// Boxed: the driver is over 200 bytes, beside an end reason of 24.
    Live(Box<Live>),
    Ended(Ended),
}

/// A session that has not ended.
#[derive(Debug)]
pub(crate) struct Live {
    driver: Driver,
    /// Told when the session ends, so the server ends with it.
    on_end: oneshot::Sender<Ended>,
}

/// Why a session ended.
#[derive(Clone, Debug)]
pub(crate) enum Ended {
    /// The editor panicked, with this message and location.
    Panic(String),
    /// An action asked the editor to quit, as File ▸ Quit or Ctrl+Q do.
    Quit,
    /// The window the session drove closed.
    Closed,
}

/// A failed call, in words that tell the agent how to correct it.
#[derive(Debug)]
pub(crate) struct ToolError(pub(crate) String);

impl SessionThread {
    /// Start the editor's thread on the session `make` makes there, and the receiver told when
    /// that session ends; or the error that kept the session from starting. It blocks until the
    /// session starts, so an async caller runs it on a blocking thread.
    pub(crate) fn start(
        make: impl FnOnce() -> Result<Driver, DriverError> + Send + 'static,
    ) -> Result<(Self, oneshot::Receiver<Ended>), DriverError> {
        let (jobs, requests) = mpsc::channel(WAITING_JOBS);
        let (started, startup) = oneshot::channel();
        let (on_end, ended) = oneshot::channel();
        let handle = threads::spawn("darkroom-mcp-session", move || {
            let _panics = PanicRecord::register();
            let driver = match make() {
                Ok(driver) => driver,
                Err(error) => {
                    started.send(Err(error)).ok();
                    return;
                }
            };
            started.send(Ok(())).ok();
            serve(requests, Session::Live(Box::new(Live { driver, on_end })));
        });
        let thread = Self {
            jobs,
            _thread: Joined(Some(handle)),
        };
        // A thread that ends before it answers panicked, which the hook printed.
        match startup.blocking_recv() {
            Ok(Ok(())) => Ok((thread, ended)),
            Ok(Err(error)) => Err(error),
            Err(oneshot::error::RecvError { .. }) => {
                panic!("the editor's thread panicked as the session started")
            }
        }
    }

    /// End the session because its window closed, and wait until its driver, with the session
    /// directory, is gone: the process ends with the window, and would leave the directory
    /// behind. A session that already ended has nothing to end.
    pub(crate) fn close(&self) {
        let (reply, answer) = oneshot::channel();
        let job: Job = Box::new(|session| {
            session.end(Ended::Closed);
            Ok(Vec::new())
        });
        if self.jobs.blocking_send(Request { job, reply }).is_ok() {
            answer.blocking_recv().ok();
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
            .await
            .expect("the editor's thread lives as long as the server");
        answer
            .await
            .expect("the editor's thread answers every job it takes")
    }
}

/// An offscreen driver of `config` on `document`, or on a new document for none, whose logs `log`
/// keeps.
pub(crate) fn launch(
    config: SessionConfig,
    log: LogRing,
    document: Option<&Path>,
) -> Result<Driver, DriverError> {
    let mut driver = Driver::new(config, log)?;
    if let Some(document) = document {
        driver.open(document, OpenMode::InPlace)?;
    }
    Ok(driver)
}

/// Take jobs until the server drops its end, each inside the panic edge.
fn serve(mut requests: mpsc::Receiver<Request>, mut session: Session) {
    while let Some(Request { job, reply }) = requests.blocking_recv() {
        let result = match panic::catch_unwind(AssertUnwindSafe(|| job(&mut session))) {
            Ok(Ok(content)) => CallToolResult::success(content),
            Ok(Err(ToolError(message))) => CallToolResult::error(vec![ContentBlock::text(message)]),
            Err(_) => {
                let panic = take_panic();
                session.end(Ended::Panic(panic.clone()));
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
        match self {
            Self::Live(live) => Ok(&mut live.driver),
            Self::Ended(Ended::Panic(panic)) => Err(ToolError(format!(
                "the session ended when the editor panicked: {panic}; the server is ending"
            ))),
            Self::Ended(Ended::Quit) => Err(ToolError(
                "the session ended when an action quit the editor; the server is ending"
                    .to_string(),
            )),
            Self::Ended(Ended::Closed) => Err(ToolError(
                "the session ended when its window closed; the server is ending".to_string(),
            )),
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
        self.end(Ended::Quit);
    }

    /// End a live session for `why`, and tell the server once its driver is dropped. A drop that
    /// panics ends it by that panic, unless a panic ended it already: that state is broken, and
    /// the first panic is the cause. A session already ended keeps the reason it ended for.
    fn end(&mut self, why: Ended) {
        let Live { driver, on_end } = match mem::replace(self, Self::Ended(why.clone())) {
            Self::Live(live) => *live,
            ended @ Self::Ended(_) => {
                *self = ended;
                return;
            }
        };
        let dropped = panic::catch_unwind(AssertUnwindSafe(move || drop(driver)));
        let why = match why {
            Ended::Quit | Ended::Closed if dropped.is_err() => Ended::Panic(take_panic()),
            why => why,
        };
        *self = Self::Ended(why.clone());
        // The server already gone: no one waits for the end.
        on_end.send(why).ok();
    }
}

/// The panic record's entry for the thread that registered it, removed when that thread ends.
#[derive(Debug)]
struct PanicRecord(ThreadId);

impl PanicRecord {
    /// Record this thread's panics from now on, installing the hook on the first call.
    fn register() -> Self {
        let id = thread::current().id();
        let panics = PANICS.get_or_init(|| {
            let previous = panic::take_hook();
            panic::set_hook(Box::new(move |info| {
                record(info);
                previous(info);
            }));
            Mutex::default()
        });
        panics.lock_unpoisoned().insert(id, None);
        Self(id)
    }
}

impl Drop for PanicRecord {
    fn drop(&mut self) {
        if let Some(panics) = PANICS.get() {
            panics.lock_unpoisoned().remove(&self.0);
        }
    }
}

/// The panic the hook recorded on this thread, a registered one, since the last take.
fn take_panic() -> String {
    PANICS
        .get()
        .and_then(|panics| {
            panics
                .lock_unpoisoned()
                .get_mut(&thread::current().id())
                .and_then(Option::take)
        })
        .unwrap_or_else(|| "a panic the hook did not see".to_string())
}

/// Keep the message and location of a panic on a registered thread: the payload `catch_unwind`
/// returns has no location.
fn record(info: &PanicHookInfo<'_>) {
    let Some(panics) = PANICS.get() else {
        return;
    };
    if let Some(slot) = panics.lock_unpoisoned().get_mut(&thread::current().id()) {
        let message = info.payload_as_str().unwrap_or("a panic with no message");
        let location = info
            .location()
            .map_or_else(String::new, |location| format!(" at {location}"));
        *slot = Some(format!("{message}{location}"));
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
        Self(ErrorChain::new(&error).to_string())
    }
}
