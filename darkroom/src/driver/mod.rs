//! The editor driver: the whole editor, played and read as a person at its window would. It opens
//! a project, plays input into the window, waits for it to settle, and reads back what it shows.
//! The window is offscreen, on the driver's thread, or the desktop one, on the main thread.
//! `darkroom/AGENTS.md`, "The editor harness", holds the decisions.

pub mod action;
mod backend;
pub mod error;
pub mod locator;
pub mod log_ring;
mod offscreen;
pub mod shot;
pub mod state;
mod window_backend;
pub mod window_link;

use std::path::{self, Path, PathBuf};
use std::time::{Duration, Instant};

use common::{TempDir, env, fs};
use glam::UVec2;
use serde::Serialize;

use crate::core::io::document as io_document;
use crate::driver::action::Action;
use crate::driver::backend::Backend;
use crate::driver::error::{ActFailure, DriverError};
use crate::driver::log_ring::LogRing;
use crate::driver::offscreen::Offscreen;
use crate::driver::shot::{Region, Shot};
use crate::driver::state::{Detail, State, UiList};
use crate::driver::window_backend::WindowBackend;
use crate::driver::window_link::WindowLink;

/// Frames an action may run before it counts as unsettled: ten seconds of UI time.
const SETTLE_FRAMES: u32 = 600;
/// Real time an action may wait for the app's background work, a preview's conversion, to land.
const BACKGROUND_WAIT: Duration = Duration::from_secs(30);
/// The largest project folder a session copies into its directory, which sits in RAM on a tmpfs
/// `/tmp`.
pub const COPY_LIMIT: u64 = 256 * 1024 * 1024;

/// Which display an offscreen driver renders through, chosen when it is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayKind {
    /// A headless GPU device and an offscreen texture: pixels, and screenshots of them. Tests
    /// take `TestGpu`.
    Gpu,
    /// palantir's deviceless frontend: input, layout and hit tests, no pixels. Test code.
    #[cfg(feature = "internals")]
    Cpu,
    /// [`DisplayKind::Gpu`] on the one device palantir's `headless_test_gpu` leases to the test
    /// process, which panics where no adapter is. Test code: tests run in parallel in one
    /// process, and a device created on one Vulkan instance while another enumerated its adapters
    /// crashed in the Vulkan loader on lavapipe (CI run 40528808017).
    #[cfg(feature = "internals")]
    TestGpu,
}

/// Where [`Driver::open`] opens a project. `darkroom/AGENTS.md`, "The editor harness", "A
/// project opens in place".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    /// The file where it is, as a person's Open does: saves and its cache reach the original.
    InPlace,
    /// A copy of its folder in the session directory, so nothing the session does reaches the
    /// original. Refused for a folder over [`COPY_LIMIT`], one holding a link to a folder, and a
    /// document whose relative paths climb out of it.
    Copy,
}

/// How an offscreen driver's window is made.
#[derive(Clone, Copy, Debug)]
pub struct SessionConfig {
    pub display: DisplayKind,
    /// Physical pixels.
    pub size: UVec2,
    /// Physical pixels per logical pixel.
    pub scale: f32,
}

/// The editor a driver plays, and the session directory its copies and shots live in.
#[derive(Debug)]
pub struct Driver {
    backend: Backend,
    /// The session's own directory, which is RAM on a tmpfs `/tmp`.
    session: TempDir,
    log: LogRing,
    /// The runs the worker had ended when the last `act` began, which a run it starts ends past.
    runs_before: u64,
    /// Shots taken, which numbers the next one's file.
    shots: u32,
}

/// What an action changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ActReport {
    /// Frames the actions ran.
    pub frames: u32,
    /// The selected nodes after them, by name.
    pub selection: Vec<String>,
    pub dirty: bool,
    /// The focused widget's text after them.
    pub focus: Option<String>,
}

impl Driver {
    /// A driver of an offscreen editor on an empty document, whose logs `log` keeps once
    /// [`crate::init_tracing`] feeds it, or [`DriverError::NoAdapter`] for [`DisplayKind::Gpu`] on
    /// a machine with no usable adapter.
    pub fn new(config: SessionConfig, log: LogRing) -> Result<Self, DriverError> {
        let offscreen = Offscreen::new(config)?;
        Self::over(Backend::Offscreen(Box::new(offscreen)), log)
    }

    /// A driver of the editor in the desktop window `link` reaches, whose logs `log` keeps.
    pub fn window(link: WindowLink, log: LogRing) -> Result<Self, DriverError> {
        Self::over(Backend::Window(WindowBackend::new(link)), log)
    }

    fn over(backend: Backend, log: LogRing) -> Result<Self, DriverError> {
        let temp = env::temp_dir();
        let session =
            TempDir::create_in(&temp, "driver").map_err(|source| io_error(&temp, source))?;
        let mut driver = Self {
            backend,
            session,
            log,
            runs_before: 0,
            shots: 0,
        };
        driver.backend.settle("the launch")?;
        Ok(driver)
    }

    /// The directory this session's copies and files live in, removed when the driver drops.
    pub fn session_dir(&self) -> &Path {
        self.session.path()
    }

    /// Open the project at `document`, where `mode` says. A project that does not open leaves
    /// the editor as it was, as a person's Open does.
    pub fn open(&mut self, document: &Path, mode: OpenMode) -> Result<(), DriverError> {
        // Absolute, as a native dialog answers: memory holds every path absolute, and the backend
        // knows the document opened by its path.
        let document = path::absolute(document).map_err(|source| io_error(document, source))?;
        let opened = match mode {
            OpenMode::InPlace => document,
            OpenMode::Copy => self.copy_project(&document)?,
        };
        self.reopen(Some(opened), "the open")
    }

    /// A new unsaved document, which a Save As names, in place of the open one.
    pub fn new_document(&mut self) -> Result<(), DriverError> {
        self.reopen(None, "the new document")
    }

    /// An offscreen editor that quit opens again as a new app; a window that quit has closed.
    fn reopen(&mut self, document: Option<PathBuf>, step: &str) -> Result<(), DriverError> {
        self.backend.open(document)?;
        self.backend.settle(step)?;
        Ok(())
    }

    /// Play `actions` in order, settling after each; the first that fails stops the rest. A run
    /// any of them starts is one [`Self::wait_run`] waits for.
    pub fn act(&mut self, actions: &[Action]) -> Result<ActReport, ActFailure> {
        let failed = |ran| move |error| ActFailure { ran, error };
        self.backend.running().map_err(failed(0))?;
        self.runs_before = self
            .backend
            .with_app(|app, _| app.run_state().runs_ended())
            .map_err(failed(0))?;
        let mut frames = 0;
        for (ran, action) in actions.iter().enumerate() {
            frames += self.play(action).map_err(failed(ran))?;
        }
        // Not `Self::state`, whose report takes the log lines a later `state` call is owed.
        let state = self
            .backend
            .with_app(|app, ui| State::of(app, ui, Detail::Concise, None, Vec::new()))
            .map_err(failed(actions.len()))?;
        Ok(ActReport {
            frames,
            selection: state.selection,
            dirty: state.dirty,
            focus: state.focus,
        })
    }

    /// The editor as its last frame shows it, keeping the nodes whose name contains `nodes`, when
    /// given.
    pub fn state(&mut self, detail: Detail, nodes: Option<&str>) -> Result<State, DriverError> {
        let log = self.log.drain();
        let nodes = nodes.map(str::to_owned);
        self.backend
            .with_app(move |app, ui| State::of(app, ui, detail, nodes.as_deref(), log))
    }

    /// The visible interactive widgets whose text contains `filter`, at most `limit`.
    pub fn ui(&mut self, filter: Option<&str>, limit: usize) -> Result<UiList, DriverError> {
        Ok(UiList::of(&self.backend.widgets(filter)?, limit))
    }

    /// A picture of `region` as a later frame paints it, scaled so that its longer side is at
    /// most `max_side` pixels, and written to `shots/<n>-<region>.png` in the session directory.
    ///
    /// # Panics
    ///
    /// When `max_side` is 0.
    pub fn shot(&mut self, region: &Region, max_side: u32) -> Result<Shot, DriverError> {
        assert!(max_side > 0, "a shot has at least one pixel a side");
        self.backend.running()?;
        self.shots += 1;
        let dir = self.session.join("shots");
        let path = dir.join(format!("{:03}-{}.png", self.shots, region.slug()));
        let capture = self.backend.shot(region, max_side);
        // The session settles even when the shot fails, so the next call starts from a settled
        // frame.
        self.backend.settle(&format!("the shot of {region:?}"))?;
        let shot = Shot::new(capture?, path);
        fs::create_dir_all(&dir).map_err(|source| io_error(&dir, source))?;
        fs::write(&shot.path, &shot.png).map_err(|source| io_error(&shot.path, source))?;
        Ok(shot)
    }

    /// Wait until a run that began after the last `act` began has ended, the worker has no run
    /// in flight, and the editor settles, its previews landed. A run the worker has not started
    /// yet reads as idle, so the run's end, and not the worker's idleness, ends the wait.
    pub fn wait_run(&mut self, timeout: Duration) -> Result<(), DriverError> {
        self.backend.running()?;
        let deadline = Instant::now() + timeout;
        let before = self.runs_before;
        loop {
            self.backend.settle("the run")?;
            let (executing, ended) = self.backend.with_app(|app, _| {
                let run = app.run_state();
                (run.activity().is_executing(), run.runs_ended())
            })?;
            if ended > before && !executing && !self.backend.wake_pending() {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(DriverError::RunTimeout {
                    seconds: timeout.as_secs_f32(),
                });
            }
            // The worker posts on real time and wakes the app when it does.
            self.backend.wait_wake(deadline);
        }
    }

    /// Play one action and settle, as palantir plays it: every locator resolves before any input
    /// goes in, and nothing between a press or a modifier and its release can fail, so an error
    /// never leaves a button or a modifier held for the next action.
    fn play(&mut self, action: &Action) -> Result<u32, DriverError> {
        let step = format!("{action:?}");
        match action {
            Action::Play(action) => self.backend.play(action, &step),
            Action::Answer(answer) => {
                let answer = answer.clone();
                self.backend
                    .with_app(move |app, _| app.dialogs_mut().queue(answer))?;
                self.backend.settle(&step)
            }
        }
    }

    /// Copy the folder of `document` into the session directory, refusing one over
    /// [`COPY_LIMIT`]; the copy of `document` is returned.
    fn copy_project(&self, document: &Path) -> Result<PathBuf, DriverError> {
        let folder = document
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        let (Some(folder), Some(name)) = (folder, document.file_name()) else {
            return Err(DriverError::NoFolder {
                path: document.to_path_buf(),
            });
        };
        // A document the load cannot read is the open's to report, as it is for one in place.
        if let Ok(paths) = io_document::paths_leaving_folder(document)
            && !paths.is_empty()
        {
            return Err(DriverError::PathsLeaveFolder {
                document: document.to_path_buf(),
                paths,
            });
        }
        let files = ProjectFiles::of(folder)?;
        if files.bytes > COPY_LIMIT {
            return Err(DriverError::FolderTooLarge {
                folder: folder.to_path_buf(),
                bytes: files.bytes,
                limit: COPY_LIMIT,
            });
        }
        let copy = self.session.join("project");
        if fs::exists(&copy) {
            fs::remove_dir_all(&copy).map_err(|source| io_error(&copy, source))?;
        }
        files.copy(folder, &copy)?;
        Ok(copy.join(name))
    }
}

/// What a project folder holds, by path relative to it: the folders to make and the files to
/// copy, and the files' bytes. A link to a file counts and copies as the file it names.
#[derive(Debug, Default)]
struct ProjectFiles {
    folders: Vec<PathBuf>,
    files: Vec<PathBuf>,
    bytes: u64,
}

impl ProjectFiles {
    /// Walk `root`, refusing a link to a folder, which could hold the folder itself.
    fn of(root: &Path) -> Result<Self, DriverError> {
        let mut found = Self::default();
        let mut pending = vec![PathBuf::new()];
        while let Some(relative) = pending.pop() {
            let dir = root.join(&relative);
            for entry in fs::read_dir(&dir).map_err(|source| io_error(&dir, source))? {
                let entry = entry.map_err(|source| io_error(&dir, source))?;
                let path = entry.path();
                let inner = relative.join(entry.file_name());
                let link = fs::symlink_metadata(&path).map_err(|source| io_error(&path, source))?;
                let metadata = fs::metadata(&path).map_err(|source| io_error(&path, source))?;
                if metadata.is_dir() {
                    if link.file_type().is_symlink() {
                        return Err(DriverError::LinkedFolder { path });
                    }
                    found.folders.push(inner.clone());
                    pending.push(inner);
                } else {
                    found.bytes += metadata.len();
                    found.files.push(inner);
                }
            }
        }
        Ok(found)
    }

    /// Make the folders under `to` and copy the files from `from` into them.
    fn copy(&self, from: &Path, to: &Path) -> Result<(), DriverError> {
        fs::create_dir_all(to).map_err(|source| io_error(to, source))?;
        for folder in &self.folders {
            let target = to.join(folder);
            fs::create_dir_all(&target).map_err(|source| io_error(&target, source))?;
        }
        for file in &self.files {
            let source_path = from.join(file);
            fs::copy(&source_path, to.join(file))
                .map_err(|source| io_error(&source_path, source))?;
        }
        Ok(())
    }
}

fn io_error(path: &Path, source: std::io::Error) -> DriverError {
    DriverError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::driver::Driver;
    use crate::driver::backend::Backend;
    use crate::driver::offscreen::Offscreen;
    use crate::gui::app::App;

    impl Driver {
        /// The offscreen editor, for a test that reads what no report says.
        pub(crate) fn offscreen(&mut self) -> &mut Offscreen {
            match &mut self.backend {
                Backend::Offscreen(offscreen) => offscreen,
                Backend::Window(_) => unreachable!("a test drives an offscreen editor"),
            }
        }

        pub(crate) fn app(&self) -> &App {
            match &self.backend {
                Backend::Offscreen(offscreen) => offscreen.app(),
                Backend::Window(_) => unreachable!("a test drives an offscreen editor"),
            }
        }
    }
}

#[cfg(test)]
mod tests;
