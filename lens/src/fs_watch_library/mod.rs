//! Filesystem watcher node library and its per-node watcher state.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::Notify;
use tokio::time::timeout;

use scenarium::{DataType, FsPathConfig, FsPathMode, StaticValue};
use scenarium::{EventLambda, Func, FuncId, FuncInput, FuncLambda, FuncOutput, Library};

const WATCH_DIRECTORY_FUNC_ID: FuncId = FuncId::from_u128(0x1318c24c2ac74a9aa454281bdbdc4ffc);

/// Per-node state shared between the func lambda (which builds the OS watcher)
/// and the `changed` event lambda (which awaits the next notification).
struct WatchState {
    path: String,
    recursive: bool,
    /// Trailing-edge quiet window: after the first change the event lambda keeps
    /// absorbing follow-up changes until the directory is silent this long, then
    /// fires once. Collapses the burst of low-level events one user action (paste,
    /// save) produces into a single fire. Zero disables it.
    debounce: Duration,
    /// Pulsed by the watcher's callback thread on every filesystem event; the
    /// event lambda parks on `notified()`. `notify_one` keeps a single permit
    /// when no waiter is parked, so a change between fires is never missed and a
    /// burst of changes collapses into one wake-up.
    signal: Arc<Notify>,
    /// RAII guard: held only to keep the OS watch open — dropping it removes the
    /// watch. Never read.
    _watcher: RecommendedWatcher,
}

impl WatchState {
    fn new(path: &str, recursive: bool, debounce: Duration) -> anyhow::Result<Self> {
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("failed to inspect watch directory {path:?}"))?;
        anyhow::ensure!(metadata.is_dir(), "watch path is not a directory: {path:?}");

        let signal = Arc::new(Notify::new());
        let callback_signal = signal.clone();
        let owned_path = path.to_string();
        let callback_path = owned_path.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
                Ok(event) if is_content_change(&event.kind) => callback_signal.notify_one(),
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    path = %callback_path,
                    %error,
                    "filesystem watcher backend error"
                ),
            })?;
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher.watch(Path::new(path), mode)?;
        Ok(Self {
            path: owned_path,
            recursive,
            debounce,
            signal,
            _watcher: watcher,
        })
    }
}

impl std::fmt::Debug for WatchState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchState")
            .field("path", &self.path)
            .field("recursive", &self.recursive)
            .field("debounce", &self.debounce)
            .finish_non_exhaustive()
    }
}

/// Whether an event is a change to the directory's *contents* — a new file, a
/// removal, a rename, or a data write.
///
/// We subscribe to exactly `Create`, `Remove`, and `Modify`, and drop everything
/// else: `Access` (reads/opens/closes) and the uncategorized `Any`/`Other`
/// catch-alls. The one carve-out is metadata-only `Modify`s (access/write time,
/// permissions, ownership, xattrs) — that's where an access-time bump lands, so
/// it's filtered too. `Modify(Any)` is still kept: some backends (macOS
/// FSEvents) report a real write that coarsely, and dropping it would swallow
/// genuine changes.
fn is_content_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_) => true,
        _ => false,
    }
}

fn directory_type() -> DataType {
    DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::Directory)))
}

/// A node that passes a directory path through unchanged and fires a `changed`
/// event whenever the directory's contents change on disk.
pub fn fs_watch_library() -> Library {
    let mut library = Library::default();

    library.add(
        Func::new(WATCH_DIRECTORY_FUNC_ID, "Watch Directory")
            .category("System")
            .description(
                "Passes a directory through unchanged and fires `Changed` when files are added, \
                 removed, renamed, or written — ignoring access-time and other metadata-only events.",
            )
            .input(
                FuncInput::required("Directory", directory_type())
                    .description("Directory to watch and pass through.")
                    .const_only(),
            )
            .input(
                FuncInput::required("Recursive", DataType::Bool)
                    .description("Watch subdirectories too, not just the top level.")
                    .default(true)
                    .const_only(),
            )
            .input(
                FuncInput::required("Debounce (ms)", DataType::Int)
                    .description(
                        "Quiet window after a change before firing, collapsing a burst into one \
                         event. 0 disables debouncing.",
                    )
                    .default(1000i64)
                    .const_only(),
            )
            .output(
                FuncOutput::new("Directory", directory_type())
                    .description("The same directory, re-emitted on each change."),
            )
            .event(
                "Changed",
                EventLambda::new(|state| {
                    Box::pin(async move {
                        let watch = state
                            .lock()
                            .await
                            .get::<WatchState>()
                            .map(|w| (w.signal.clone(), w.debounce));
                        match watch {
                            Some((signal, debounce)) => {
                                // Wait for the first change of a burst...
                                signal.notified().await;
                                // ...then absorb follow-up changes until the
                                // directory stays quiet for `debounce`, so one
                                // user action fires once instead of N times.
                                while !debounce.is_zero()
                                    && timeout(debounce, signal.notified()).await.is_ok()
                                {}
                            }
                            // No watcher (e.g. empty/invalid path): never fire.
                            None => std::future::pending::<()>().await,
                        }
                    })
                }),
            )
            .lambda(FuncLambda::new(
                move |_ctx, _state, event_state, inputs, _demand, outputs| {
                    Box::pin(async move {
                        assert_eq!(inputs.len(), 3);
                        assert_eq!(outputs.len(), 1);
                        let path = inputs[0]
                            .value
                            .as_fs_path()
                            .expect("directory input type is validated at the compile boundary")
                            .to_string();
                        let recursive = inputs[1]
                            .value
                            .as_bool()
                            .expect("recursive input type is validated at the compile boundary");
                        let debounce = Duration::from_millis(
                            inputs[2]
                                .value
                                .as_i64()
                                .expect(
                                    "debounce input type is validated at the compile boundary",
                                )
                                .max(0) as u64,
                        );

                        if !path.is_empty() {
                            let needs_rebuild = event_state
                                .lock()
                                .await
                                .get::<WatchState>()
                                .is_none_or(|w| w.path != path || w.recursive != recursive);
                            if needs_rebuild {
                                let watch_path = path.clone();
                                // Watch registration can block on slow or network filesystems.
                                let replacement = tokio::task::spawn_blocking(move || {
                                    WatchState::new(&watch_path, recursive, debounce)
                                })
                                .await
                                .expect("filesystem watcher setup task panicked");
                                let mut guard = event_state.lock().await;
                                guard.clear();
                                guard.set(replacement?);
                            } else {
                                // Debounce is consumer-side — retune without
                                // tearing down the live OS watch.
                                event_state
                                    .lock()
                                    .await
                                    .get_mut::<WatchState>()
                                    .unwrap()
                                    .debounce = debounce;
                            }
                        } else {
                            // Cleared path: tear the previous watcher down, or the old
                            // directory keeps firing `Changed` (and holding its OS watch)
                            // while the node outputs an empty path.
                            event_state.lock().await.clear();
                        }

                        outputs[0] = StaticValue::FsPath(path).into();
                        Ok(())
                    })
                },
            )),
    );

    library
}

#[cfg(test)]
mod tests;
