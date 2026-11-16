//! [`LibraryScanner`]: the library as the last scan read it, kept current off the frame.
//!
//! # Decisions
//!
//! - **The folder is watched through `notify`**, the workspace's watcher in `common::fs`, so a
//!   library file another window writes is listed again with no restart. Refused: a poll of the
//!   folder's times, which reads the disk every frame or answers late.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use common::{ErrorChain, fs, thread};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::core::shared_library::{SharedLibrary, scan};
use crate::core::wake::Wake;

/// The library folder's scan: the library the last one read, the one in flight, and the watch
/// that asks for the next when the folder changes. One scan runs at a time, and changes while it
/// runs coalesce into one more.
pub(crate) struct LibraryScanner {
    folder: Option<PathBuf>,
    library: SharedLibrary,
    pending: Option<Receiver<SharedLibrary>>,
    /// Set by the watch when the folder changes; the next poll starts a scan.
    changed: Arc<AtomicBool>,
    /// Held so the watch lasts while the folder is followed.
    watcher: Option<RecommendedWatcher>,
    wake: Wake,
}

impl fmt::Debug for LibraryScanner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryScanner")
            .field("folder", &self.folder)
            .field("library", &self.library)
            .field("scanning", &self.pending.is_some())
            .finish_non_exhaustive()
    }
}

impl LibraryScanner {
    pub(crate) fn new(wake: Wake) -> Self {
        Self {
            folder: None,
            library: SharedLibrary::default(),
            pending: None,
            changed: Arc::new(AtomicBool::new(false)),
            watcher: None,
            wake,
        }
    }

    /// The library the last finished scan read; empty before the first, and for no folder.
    pub(crate) const fn library(&self) -> &SharedLibrary {
        &self.library
    }

    /// Follow `folder`, the preference as it stands: a new folder starts a scan and a watch of
    /// it, and none drops both and lists nothing.
    pub(crate) fn follow(&mut self, folder: Option<&Path>) {
        if self.folder.as_deref() == folder {
            return;
        }
        self.folder = folder.map(Path::to_path_buf);
        // Emptied, a generation on, so no value cached off the old folder reads as current.
        self.library = SharedLibrary {
            generation: self.library.generation + 1,
            ..SharedLibrary::default()
        };
        self.pending = None;
        self.watcher = folder.and_then(|folder| self.watch(folder));
        self.changed.store(folder.is_some(), Ordering::Relaxed);
    }

    /// Take a finished scan in and start the one the folder's changes asked for. Once a frame;
    /// `true` when the library changed.
    pub(crate) fn poll(&mut self) -> bool {
        let mut landed = false;
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(mut library) => {
                    library.generation = self.library.generation + 1;
                    self.library = library;
                    self.pending = None;
                    landed = true;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.pending = None,
            }
        }
        if self.pending.is_none()
            && let Some(folder) = &self.folder
            && self.changed.swap(false, Ordering::Relaxed)
        {
            self.pending = Some(start_scan(folder.clone(), Arc::clone(&self.wake)));
        }
        landed
    }

    /// A watch of `folder` that marks it changed and wakes the window; `None` when the system
    /// gives none, as then the folder is read at start alone.
    fn watch(&self, folder: &Path) -> Option<RecommendedWatcher> {
        let (changed, wake) = (Arc::clone(&self.changed), Arc::clone(&self.wake));
        let mut watcher = fs::watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok() {
                changed.store(true, Ordering::Relaxed);
                wake();
            }
        })
        .inspect_err(|error| {
            tracing::warn!(error = %ErrorChain::new(error), "the library folder is not watched");
        })
        .ok()?;
        watcher
            .watch(folder, RecursiveMode::NonRecursive)
            .inspect_err(|error| {
                tracing::warn!(error = %ErrorChain::new(error), "the library folder is not watched");
            })
            .ok()?;
        Some(watcher)
    }
}

/// Scan `folder` on a thread of its own, posting the library it reads and waking the window.
fn start_scan(folder: PathBuf, wake: Wake) -> Receiver<SharedLibrary> {
    let (post, landed) = mpsc::channel();
    thread::spawn("library scan", move || {
        // A scanner dropped meanwhile reads no result.
        post.send(scan(&folder)).ok();
        wake();
    });
    landed
}
