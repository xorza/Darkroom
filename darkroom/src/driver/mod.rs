//! The editor driver: the whole editor, played and read as a person at its window would. It opens
//! a project, plays input into the window, waits for it to settle, and reads back what it shows.
//! The window is offscreen, on the driver's thread, or the desktop one, on the main thread.
//! `darkroom/AGENTS.md`, "The editor harness", holds the decisions.

pub mod action;
mod backend;
pub mod display;
pub mod error;
pub mod locator;
pub mod log_ring;
mod offscreen;
pub mod shot;
pub mod state;
mod window_backend;
pub mod window_link;

use std::path::{self, Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use common::{env, fs};
use glam::{UVec2, Vec2};
use palantir::{InputEvent, Key, KeyText, Modifiers, PointerButton};
use serde::Serialize;
use tracing::subscriber::{self, DefaultGuard};
use tracing_subscriber::layer::SubscriberExt as _;

use crate::core::io::document as io_document;
use crate::driver::action::{Action, Chord, path};
use crate::driver::backend::Backend;
use crate::driver::error::DriverError;
use crate::driver::locator::Locator;
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
    /// A headless GPU device and an offscreen texture: pixels, and screenshots of them.
    Gpu,
    /// palantir's deviceless frontend: input, layout and hit tests, no pixels. Test code.
    #[cfg(feature = "internals")]
    Cpu,
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
    session: SessionDir,
    log: LogRing,
    /// An offscreen driver's log layer, this thread's default while the driver lives; the window
    /// logs on the main thread, into the layer [`crate::init_driven_tracing`] installs.
    _log_guard: Option<DefaultGuard>,
    /// How soon a second press makes a double click, which separate clicks wait out.
    double_click: Duration,
    /// Whether the last pointer step was a click, so the next one waits out the double-click
    /// window.
    clicked: bool,
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
    /// A driver of an offscreen editor on an empty document, or [`DriverError::NoAdapter`] for
    /// [`DisplayKind::Gpu`] on a machine with no usable adapter.
    ///
    /// One per thread at a time: its log layer is the thread's default subscriber while it lives,
    /// and a second driver on the thread would take the first one's lines.
    ///
    /// # Panics
    ///
    /// When the session directory cannot be made.
    pub fn new(config: SessionConfig) -> Result<Self, DriverError> {
        let log = LogRing::default();
        let log_guard = subscriber::set_default(tracing_subscriber::registry().with(log.clone()));
        let offscreen = Offscreen::new(config)?;
        Self::over(
            Backend::Offscreen(Box::new(offscreen)),
            log,
            Some(log_guard),
        )
    }

    /// A driver of the editor in the desktop window `link` reaches, whose logs `log` keeps.
    ///
    /// # Panics
    ///
    /// When the session directory cannot be made.
    pub fn window(link: WindowLink, log: LogRing) -> Result<Self, DriverError> {
        Self::over(Backend::Window(WindowBackend::new(link)), log, None)
    }

    fn over(
        mut backend: Backend,
        log: LogRing,
        log_guard: Option<DefaultGuard>,
    ) -> Result<Self, DriverError> {
        let double_click = backend.read(|_, ui| ui.double_click_window())?;
        let mut driver = Self {
            backend,
            session: SessionDir::new(),
            log,
            _log_guard: log_guard,
            double_click,
            clicked: false,
            runs_before: 0,
            shots: 0,
        };
        driver.backend.settle("the launch")?;
        Ok(driver)
    }

    /// The directory this session's copies and files live in, removed when the driver drops.
    pub fn session_dir(&self) -> &Path {
        &self.session.path
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
        self.clicked = false;
        self.backend.settle(step)?;
        Ok(())
    }

    /// Play `actions` in order, settling after each.
    pub fn act(&mut self, actions: &[Action]) -> Result<ActReport, DriverError> {
        self.backend.running()?;
        self.runs_before = self.backend.read(|app, _| app.run_state().runs_ended)?;
        let mut frames = 0;
        for action in actions {
            frames += self.play(action)?;
        }
        // Not `Self::state`, whose report takes the log lines a later `state` call is owed.
        let state = self
            .backend
            .read(|app, ui| State::of(app, ui, Detail::Concise, None, Vec::new()))?;
        Ok(ActReport {
            frames,
            selection: state.selection,
            dirty: state.dirty,
            focus: state.focus,
        })
    }

    /// The editor as its last frame shows it; a detailed report keeps the nodes whose name
    /// contains `nodes`.
    pub fn state(&mut self, detail: Detail, nodes: Option<&str>) -> Result<State, DriverError> {
        let log = self.log.drain();
        let nodes = nodes.map(str::to_owned);
        self.backend
            .read(move |app, ui| State::of(app, ui, detail, nodes.as_deref(), log))
    }

    /// The visible interactive widgets whose text contains `filter`, at most `limit`.
    pub fn ui(&mut self, filter: Option<&str>, limit: usize) -> Result<UiList, DriverError> {
        let filter = filter.map(str::to_owned);
        self.backend
            .read(move |_, ui| UiList::of(ui, filter.as_deref(), limit))
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
        let dir = self.session.path.join("shots");
        let path = dir.join(format!("{:03}-{}.png", self.shots, region.slug()));
        let shot = self.backend.shot(region, max_side, path);
        // The session settles even when the shot fails, so the next call starts from a settled
        // frame.
        self.backend.settle(&format!("the shot of {region:?}"))?;
        let shot = shot?;
        fs::create_dir_all(&dir).map_err(|source| io_error(&dir, source))?;
        fs::write(&shot.path, &shot.png).map_err(|source| io_error(&shot.path, source))?;
        Ok(shot)
    }

    /// Wait until a run that began after the last `act` began has ended, the worker has no run
    /// in flight, and the editor settles, its previews landed. A run the worker has not started
    /// yet reads as idle, so the run's end, and not the worker's idleness, ends the wait.
    pub fn wait_run(&mut self, timeout: Duration) -> Result<(), DriverError> {
        self.backend.running()?;
        let started = Instant::now();
        let before = self.runs_before;
        loop {
            self.backend.settle("the run")?;
            let (executing, ended) = self.backend.read(|app, _| {
                let run = app.run_state();
                (run.activity.is_executing(), run.runs_ended)
            })?;
            if ended > before && !executing && !self.backend.wake_pending() {
                return Ok(());
            }
            if started.elapsed() > timeout {
                return Err(DriverError::RunTimeout {
                    seconds: timeout.as_secs_f32(),
                });
            }
            // The worker posts on real time: give it a moment before the next frame.
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Play one action and settle. Every locator resolves before any input goes in, and nothing
    /// between a press or a modifier and its release can fail, so an error never leaves a button
    /// or a modifier held for the next action.
    fn play(&mut self, action: &Action) -> Result<u32, DriverError> {
        let step = format!("{action:?}");
        let mut frames = 0;
        let clicked = matches!(action, Action::Click { .. } | Action::DoubleClick(_));
        match action {
            Action::Move(at) => {
                let point = self.point(at)?;
                self.input(InputEvent::PointerMoved(point));
            }
            Action::Click { at, button, mods } => {
                let point = self.point(at)?;
                frames += self.hover(point, &step)?;
                self.space_clicks();
                frames += self.press_and_release(*button, *mods)?;
            }
            Action::DoubleClick(at) => {
                let point = self.point(at)?;
                frames += self.hover(point, &step)?;
                self.space_clicks();
                for _ in 0..2 {
                    frames += self.press_and_release(PointerButton::Left, Modifiers::default())?;
                }
            }
            Action::Drag {
                from,
                to,
                button,
                mods,
                steps,
            } => {
                let start = self.point(from)?;
                let end = self.point(to)?;
                frames += self.hover(start, &step)?;
                self.space_clicks();
                self.modifiers(*mods);
                self.input(InputEvent::PointerPressed(*button));
                // A frame between the steps, as a hand moves: the window's frames run on, and an
                // error is past the press only if the window closed, which releases it anyway.
                frames += self.frame()?;
                for point in path(start, end, *steps) {
                    self.input(InputEvent::PointerMoved(point));
                    frames += self.frame()?;
                }
                self.input(InputEvent::PointerReleased(*button));
                frames += self.frame()?;
                self.modifiers(Modifiers::default());
            }
            Action::Scroll { at, delta } => {
                let point = self.point(at)?;
                frames += self.hover(point, &step)?;
                self.input(InputEvent::ScrollPixels(*delta));
            }
            Action::Pinch { at, factor } => {
                // palantir drops a zoom that is not a positive factor, which would pass silently.
                if !(factor.is_finite() && *factor > 0.0) {
                    return Err(DriverError::BadPinch { factor: *factor });
                }
                let point = self.point(at)?;
                frames += self.hover(point, &step)?;
                self.input(InputEvent::Zoom(*factor));
            }
            Action::Key(chord) => {
                let chord = Chord::parse(chord)?;
                let text = match chord.key {
                    Key::Char(c) if chord.types() => KeyText::from_char(c),
                    _ => KeyText::EMPTY,
                };
                self.modifiers(chord.mods);
                self.input(InputEvent::KeyDown {
                    key: chord.key,
                    repeat: false,
                    physical: Key::Other,
                    text,
                });
                frames += self.frame()?;
                self.modifiers(Modifiers::default());
            }
            Action::Type(text) => self.input(InputEvent::ImeCommit(text)),
            Action::Answer(answer) => {
                let answer = answer.clone();
                self.backend
                    .read(move |app, _| app.dialogs_mut().queue(answer))?;
            }
        }
        self.clicked = clicked;
        Ok(frames + self.backend.settle(&step)?)
    }

    /// Move the pointer to `point` and settle, so the next press hits what it hovers.
    fn hover(&mut self, point: Vec2, step: &str) -> Result<u32, DriverError> {
        self.input(InputEvent::PointerMoved(point));
        self.backend.settle(step)
    }

    /// Press and release `button` where the pointer is, with `mods` held, a frame each.
    fn press_and_release(
        &mut self,
        button: PointerButton,
        mods: Modifiers,
    ) -> Result<u32, DriverError> {
        self.modifiers(mods);
        self.input(InputEvent::PointerPressed(button));
        let pressed = self.frame();
        self.input(InputEvent::PointerReleased(button));
        let released = self.frame();
        self.modifiers(Modifiers::default());
        Ok(pressed? + released?)
    }

    /// Wait out the double-click window after a click, so the next one starts a fresh run.
    fn space_clicks(&mut self) {
        if self.clicked {
            self.backend
                .advance(self.double_click + Duration::from_millis(1));
        }
    }

    fn modifiers(&mut self, mods: Modifiers) {
        self.input(InputEvent::ModifiersChanged(mods));
    }

    fn input(&mut self, event: InputEvent<'_>) {
        self.backend.input(event);
    }

    /// One frame that sees the input sent so far, counted.
    fn frame(&mut self) -> Result<u32, DriverError> {
        self.backend.frame()?;
        Ok(1)
    }

    fn point(&mut self, at: &Locator) -> Result<Vec2, DriverError> {
        let at = at.clone();
        self.backend.read(move |app, ui| at.point(app, ui))?
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
        let copy = self.session.path.join("project");
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

/// The session's own directory under the system's temporary directory, which is RAM on a tmpfs
/// `/tmp`; removed with everything in it when the session ends.
#[derive(Debug)]
struct SessionDir {
    path: PathBuf,
}

impl SessionDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("darkroom-driver-{}-{n}", process::id()));
        fs::create_dir_all(&path).expect("the session directory is creatable");
        Self { path }
    }
}

impl Drop for SessionDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).ok();
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
