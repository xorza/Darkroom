//! The editor harness: the whole editor with no desktop. It opens a project, plays input into its
//! window, waits for it to settle, and reads back what it shows. `darkroom/AGENTS.md`, "The
//! editor harness", holds the decisions.

pub mod action;
pub mod display;
pub mod error;
pub mod fixture;
pub mod locator;
mod log_ring;
pub mod shot;
pub mod state;
mod stepped_clock;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use common::{env, fs};
use glam::{UVec2, Vec2};
use palantir::internals::DOUBLE_CLICK_WINDOW;
use palantir::{FrameReport, InputEvent, Key, KeyText, Modifiers, PointerButton};
use serde::Serialize;
use tracing::subscriber::{self, DefaultGuard};
use tracing_subscriber::layer::SubscriberExt as _;

use crate::core::io::document as io_document;
use crate::core::io::preferences::Preferences;
use crate::core::io::preferences::preferences_store::PreferencesStore;
use crate::gui::app::app_host::AppHost;
use crate::gui::app::offscreen_signals::OffscreenSignals;
use crate::gui::app::{App, LaunchInputs};
use crate::gui::dialogs::Dialogs;
use crate::internals::harness::action::{Action, Chord, path};
use crate::internals::harness::display::Display;
use crate::internals::harness::error::HarnessError;
use crate::internals::harness::locator::Locator;
use crate::internals::harness::log_ring::LogRing;
use crate::internals::harness::shot::{Region, Shot};
use crate::internals::harness::state::{Detail, State, UiList};

/// The UI time one frame takes.
const FRAME: Duration = Duration::from_nanos(16_666_667);
/// Frames an action may run before it counts as unsettled: ten seconds of UI time.
const SETTLE_FRAMES: u32 = 600;
/// Real time an action may wait for the app's background work, a preview's conversion, to land.
const BACKGROUND_WAIT: Duration = Duration::from_secs(30);
/// The largest project folder a session copies into its directory, which sits in RAM on a tmpfs
/// `/tmp`.
pub const COPY_LIMIT: u64 = 256 * 1024 * 1024;

/// Which display a harness renders through, chosen when it is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayKind {
    /// A headless GPU device and an offscreen texture: pixels, and screenshots of them. A machine
    /// with no adapter panics.
    Gpu,
    /// palantir's deviceless frontend: input, layout and hit tests, no pixels.
    Cpu,
}

/// How a harness's window is made.
#[derive(Clone, Copy, Debug)]
pub struct SessionConfig {
    pub display: DisplayKind,
    /// Physical pixels.
    pub size: UVec2,
    /// Physical pixels per logical pixel.
    pub scale: f32,
}

/// The editor in a window no one sees, and the session directory its files live in.
///
/// One per thread at a time: its log layer is the thread's default subscriber while it lives,
/// and a second harness on the thread would take the first one's lines.
#[derive(Debug)]
pub struct Harness {
    app: App,
    display: Display,
    signals: Arc<OffscreenSignals>,
    session: SessionDir,
    log: LogRing,
    /// The log layer is this thread's default while the harness lives.
    _log_guard: DefaultGuard,
    /// Whether the last pointer step was a click, so the next one waits out the double-click
    /// window.
    clicked: bool,
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

impl Harness {
    /// A harness on an empty document.
    ///
    /// # Panics
    ///
    /// For [`DisplayKind::Gpu`] on a machine with no usable adapter, and when the session
    /// directory cannot be made.
    pub fn new(config: SessionConfig) -> Self {
        let log = LogRing::default();
        let log_guard = subscriber::set_default(tracing_subscriber::registry().with(log.clone()));
        let mut display = Display::new(config.display, config.size, config.scale);
        let signals = Arc::new(OffscreenSignals::default());
        let app = launch(&mut display, &signals, None);
        let mut harness = Self {
            app,
            display,
            signals,
            session: SessionDir::new(),
            log,
            _log_guard: log_guard,
            clicked: false,
            shots: 0,
        };
        harness
            .settle("the launch")
            .expect("an empty document settles");
        harness
    }

    /// The directory this session's copies and files live in, removed when the harness drops.
    pub fn session_dir(&self) -> &Path {
        &self.session.path
    }

    /// Open the project at `document` in a new session of the editor. Without `in_place` its
    /// folder is copied into the session directory first and the copy is opened, so nothing the
    /// session does reaches the original.
    pub fn open(&mut self, document: &Path, in_place: bool) -> Result<(), HarnessError> {
        let opened = if in_place {
            document.to_path_buf()
        } else {
            self.copy_project(document)?
        };
        // Signals of its own: the session before may have asked to quit.
        self.signals = Arc::new(OffscreenSignals::default());
        self.app = launch(&mut self.display, &self.signals, Some(opened));
        self.clicked = false;
        self.settle("the open")?;
        Ok(())
    }

    /// Give the window `size` physical pixels.
    pub fn resize(&mut self, size: UVec2) -> Result<(), HarnessError> {
        self.running()?;
        self.display.resize(size);
        self.settle("the resize")?;
        Ok(())
    }

    /// Play `actions` in order, settling after each.
    pub fn act(&mut self, actions: &[Action]) -> Result<ActReport, HarnessError> {
        self.running()?;
        let mut frames = 0;
        for action in actions {
            frames += self.play(action)?;
        }
        // Not `Self::state`, whose report takes the log lines a later `state` call is owed.
        let state = State::of(
            &self.app,
            self.display.ui(),
            Detail::Concise,
            None,
            Vec::new(),
        );
        Ok(ActReport {
            frames,
            selection: state.selection,
            dirty: state.dirty,
            focus: state.focus,
        })
    }

    /// The editor as its last frame shows it; a detailed report keeps the nodes whose name
    /// contains `nodes`.
    pub fn state(&mut self, detail: Detail, nodes: Option<&str>) -> State {
        let log = self.log.drain();
        State::of(&self.app, self.display.ui(), detail, nodes, log)
    }

    /// The visible interactive widgets whose text contains `filter`, at most `limit`.
    pub fn ui(&mut self, filter: Option<&str>, limit: usize) -> UiList {
        UiList::of(self.display.ui(), filter, limit)
    }

    /// A picture of `region` as the next frame paints it, scaled so that its longer side is at
    /// most `max_side` pixels, and written to `shots/<n>-<region>.png` in the session directory.
    ///
    /// # Panics
    ///
    /// When `max_side` is 0.
    pub fn shot(&mut self, region: &Region, max_side: u32) -> Result<Shot, HarnessError> {
        assert!(max_side > 0, "a shot has at least one pixel a side");
        self.running()?;
        if !self.display.paints() {
            return Err(HarnessError::NoGpu);
        }
        let rect = region.rect(&self.app, self.display.ui())?;
        self.display.ui().request_screenshot(rect);
        let step = format!("the shot of {region:?}");
        let mut taken = None;
        for _ in 0..SETTLE_FRAMES {
            self.frame();
            if let Some(screenshot) = self.display.ui().screenshot_taken() {
                taken = Some(screenshot.clone());
                break;
            }
        }
        let screenshot = taken.ok_or_else(|| HarnessError::Unsettled {
            step: step.clone(),
            frames: SETTLE_FRAMES,
        })?;
        self.shots += 1;
        let dir = self.session.path.join("shots");
        let path = dir.join(format!("{:03}-{}.png", self.shots, region.slug()));
        // The legend reads the frame the image landed in; the session settles even when the shot
        // fails, so the next call starts from a settled frame.
        let shot = Shot::of(&screenshot, max_side, &self.app, self.display.ui(), path);
        self.settle(&step)?;
        let shot = shot?;
        fs::create_dir_all(&dir).map_err(|source| io_error(&dir, source))?;
        fs::write(&shot.path, &shot.png).map_err(|source| io_error(&shot.path, source))?;
        Ok(shot)
    }

    /// Run frames until the worker has no run in flight and the editor settles, its previews
    /// landed.
    pub fn wait_run(&mut self, timeout: Duration) -> Result<(), HarnessError> {
        self.running()?;
        let started = Instant::now();
        loop {
            self.settle("the run")?;
            if !self.app.run_state().activity.is_executing() && !self.signals.wake_pending() {
                return Ok(());
            }
            if started.elapsed() > timeout {
                return Err(HarnessError::RunTimeout {
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
    fn play(&mut self, action: &Action) -> Result<u32, HarnessError> {
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
                frames += self.press_and_release(*button, *mods);
            }
            Action::DoubleClick(at) => {
                let point = self.point(at)?;
                frames += self.hover(point, &step)?;
                self.space_clicks();
                for _ in 0..2 {
                    frames += self.press_and_release(PointerButton::Left, Modifiers::default());
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
                frames += self.frame_count();
                for point in path(start, end, *steps) {
                    self.input(InputEvent::PointerMoved(point));
                    frames += self.frame_count();
                }
                self.input(InputEvent::PointerReleased(*button));
                frames += self.frame_count();
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
                    return Err(HarnessError::BadPinch { factor: *factor });
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
                frames += self.frame_count();
                self.modifiers(Modifiers::default());
            }
            Action::Type(text) => self.input(InputEvent::ImeCommit(text)),
            Action::Answer(answer) => {
                let Dialogs::Scripted(answers) = self.app.dialogs_mut() else {
                    unreachable!("a harness app answers its dialogs from a script");
                };
                answers.push_back(answer.clone());
            }
        }
        self.clicked = clicked;
        Ok(frames + self.settle(&step)?)
    }

    /// Move the pointer to `point` and settle, so the next press hits what it hovers.
    fn hover(&mut self, point: Vec2, step: &str) -> Result<u32, HarnessError> {
        self.input(InputEvent::PointerMoved(point));
        self.settle(step)
    }

    /// Press and release `button` where the pointer is, with `mods` held, a frame each.
    fn press_and_release(&mut self, button: PointerButton, mods: Modifiers) -> u32 {
        self.modifiers(mods);
        self.input(InputEvent::PointerPressed(button));
        self.frame();
        self.input(InputEvent::PointerReleased(button));
        self.frame();
        self.modifiers(Modifiers::default());
        2
    }

    /// Wait out the double-click window after a click, so the next one starts a fresh run.
    fn space_clicks(&mut self) {
        if self.clicked {
            self.display
                .advance(DOUBLE_CLICK_WINDOW + Duration::from_millis(1));
        }
    }

    fn modifiers(&mut self, mods: Modifiers) {
        self.input(InputEvent::ModifiersChanged(mods));
    }

    fn input(&mut self, event: InputEvent<'_>) {
        self.display.on_input(event);
    }

    fn point(&mut self, at: &Locator) -> Result<Vec2, HarnessError> {
        at.point(&self.app, self.display.ui())
    }

    /// One frame. A wake asks for a full one, as the window host's `request_repaint` does: a frame
    /// nothing asked for paints only, and runs no `update` to drain what the wake announced.
    fn frame(&mut self) -> FrameReport {
        if self.signals.take_wake() {
            self.display.ui().request_repaint();
        }
        self.display.frame(&mut self.app, FRAME)
    }

    /// Refuse to go on once the editor asked to quit, as File ▸ Quit and Ctrl+Q do: a window
    /// closes then, and the session a harness runs ends with it.
    fn running(&self) -> Result<(), HarnessError> {
        if self.signals.quit_asked() {
            Err(HarnessError::Quit)
        } else {
            Ok(())
        }
    }

    /// One frame, counted.
    fn frame_count(&mut self) -> u32 {
        self.frame();
        1
    }

    /// Run frames until one asks no repaint, the app has no wake pending, and no preview is
    /// converting. A timer wake, as a caret blink asks, does not count: the frame it would run
    /// changes nothing. A conversion runs on the converter's thread, so a quiet frame with one in
    /// flight waits in real time, bounded, for the wake that makes the next frame land it. A frame
    /// that quits the editor ends the step with [`HarnessError::Quit`]: every step ends here, so no
    /// call goes on past a quit, whatever it guards at its start.
    fn settle(&mut self, step: &str) -> Result<u32, HarnessError> {
        let unsettled = || HarnessError::Unsettled {
            step: step.to_string(),
            frames: SETTLE_FRAMES,
        };
        let deadline = Instant::now() + BACKGROUND_WAIT;
        for frames in 1..=SETTLE_FRAMES {
            let report = self.frame();
            self.running()?;
            if report.repaint_requested || self.signals.wake_pending() {
                continue;
            }
            if !self.app.run_state().previews.in_flight() {
                return Ok(frames);
            }
            // The converter's wake announces the result; the next frame, made full, lands it.
            while !self.signals.wake_pending() {
                if Instant::now() > deadline {
                    return Err(unsettled());
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        Err(unsettled())
    }

    /// Copy the folder of `document` into the session directory, refusing one over
    /// [`COPY_LIMIT`]; the copy of `document` is returned.
    fn copy_project(&self, document: &Path) -> Result<PathBuf, HarnessError> {
        let folder = document
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        let (Some(folder), Some(name)) = (folder, document.file_name()) else {
            return Err(HarnessError::NoFolder {
                path: document.to_path_buf(),
            });
        };
        // A document the load cannot read is the open's to report, as it is for one in place.
        if let Ok(paths) = io_document::paths_leaving_folder(document)
            && !paths.is_empty()
        {
            return Err(HarnessError::PathsLeaveFolder {
                document: document.to_path_buf(),
                paths,
            });
        }
        let files = ProjectFiles::of(folder)?;
        if files.bytes > COPY_LIMIT {
            return Err(HarnessError::FolderTooLarge {
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

/// An app on the harness's host, opening `document` or an empty one.
fn launch(
    display: &mut Display,
    signals: &Arc<OffscreenSignals>,
    document: Option<PathBuf>,
) -> App {
    App::new(
        display.ui(),
        AppHost::Offscreen(Arc::clone(signals)),
        LaunchInputs {
            preferences: Ok(Preferences::default()),
            store: PreferencesStore::Memory,
            document,
            dialogs: Dialogs::Scripted(VecDeque::new()),
        },
    )
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
    fn of(root: &Path) -> Result<Self, HarnessError> {
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
                        return Err(HarnessError::LinkedFolder { path });
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
    fn copy(&self, from: &Path, to: &Path) -> Result<(), HarnessError> {
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

fn io_error(path: &Path, source: std::io::Error) -> HarnessError {
    HarnessError::Io {
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
        let path = env::temp_dir().join(format!("darkroom-harness-{}-{n}", process::id()));
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
mod tests;
