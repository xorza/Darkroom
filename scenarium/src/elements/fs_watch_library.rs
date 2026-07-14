use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::Notify;
use tokio::time::timeout;

use crate::library::Library;
use crate::node::definition::FuncId;
use crate::node::definition::{Func, FuncInput, FuncOutput};
use crate::node::event::EventLambda;
use crate::node::lambda::FuncLambda;
use crate::{DataType, FsPathConfig, FsPathMode, StaticValue};

pub(crate) const WATCH_DIRECTORY_FUNC_ID: FuncId =
    FuncId::from_u128(0x1318c24c2ac74a9aa454281bdbdc4ffc);

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
    #[allow(dead_code)]
    watcher: RecommendedWatcher,
}

impl WatchState {
    fn new(path: &str, recursive: bool, debounce: Duration) -> notify::Result<Self> {
        let signal = Arc::new(Notify::new());
        let callback_signal = signal.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res
                    && is_content_change(&event.kind)
                {
                    callback_signal.notify_one();
                }
            })?;
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher.watch(Path::new(path), mode)?;
        Ok(Self {
            path: path.to_string(),
            recursive,
            debounce,
            signal,
            watcher,
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
                move |_ctx, _state, event_state, inputs, _usage, outputs| {
                    Box::pin(async move {
                        let path =
                            inputs[0].value.as_fs_path().unwrap_or_default().to_string();
                        let recursive = inputs[1].value.as_bool().unwrap_or(true);
                        let debounce = Duration::from_millis(
                            inputs[2].value.as_i64().unwrap_or(1000).max(0) as u64,
                        );

                        if !path.is_empty() {
                            let mut guard = event_state.lock().await;
                            let needs_rebuild = guard
                                .get::<WatchState>()
                                .map(|w| w.path != path || w.recursive != recursive)
                                .unwrap_or(true);
                            if needs_rebuild {
                                let watch_state = WatchState::new(&path, recursive, debounce)
                                    .map_err(anyhow::Error::from)?;
                                guard.set(watch_state);
                            } else if let Some(w) = guard.get_mut::<WatchState>() {
                                // Debounce is consumer-side — retune without
                                // tearing down the live OS watch.
                                w.debounce = debounce;
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
mod tests {
    use super::{WATCH_DIRECTORY_FUNC_ID, WatchState, fs_watch_library};
    use crate::node::definition::FuncBehavior;
    use crate::node::lambda::{FuncLambda, InvokeInput, OutputUsage};
    use crate::runtime::any_state::AnyState;
    use crate::runtime::context::ContextManager;
    use crate::runtime::shared_any_state::SharedAnyState;
    use crate::{DynamicValue, StaticValue};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;
    use tokio::time::{Duration, sleep, timeout};

    fn unique_temp_dir() -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("scenarium-watch-test-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn invoke_watch(
        lambda: &FuncLambda,
        ctx: &mut ContextManager,
        state: &mut AnyState,
        event_state: &SharedAnyState,
        path: &str,
        recursive: bool,
    ) -> DynamicValue {
        let mut inputs = [
            InvokeInput {
                value: StaticValue::FsPath(path.to_string()).into(),
            },
            InvokeInput {
                value: StaticValue::Bool(recursive).into(),
            },
            InvokeInput {
                value: StaticValue::Int(250).into(),
            },
        ];
        let usage = [OutputUsage::Needed(1)];
        let mut outputs = [DynamicValue::Unbound];
        lambda
            .invoke(ctx, state, event_state, &mut inputs, &usage, &mut outputs)
            .await
            .unwrap();
        outputs[0].clone()
    }

    async fn stored_signal(event_state: &SharedAnyState) -> Option<Arc<tokio::sync::Notify>> {
        event_state
            .lock()
            .await
            .get::<WatchState>()
            .map(|w| w.signal.clone())
    }

    #[test]
    fn registers_watch_directory_func() {
        let lib = fs_watch_library();
        let func = lib.by_name("Watch Directory").expect("func registered");

        assert_eq!(func.id, WATCH_DIRECTORY_FUNC_ID);
        assert_eq!(func.inputs.len(), 3);
        assert_eq!(func.inputs[0].name, "Directory");
        assert_eq!(func.inputs[1].name, "Recursive");
        assert_eq!(func.inputs[1].default_value, Some(StaticValue::Bool(true)));
        assert_eq!(func.inputs[2].name, "Debounce (ms)");
        assert_eq!(func.inputs[2].default_value, Some(StaticValue::Int(1000)));
        assert_eq!(func.outputs.len(), 1);
        assert_eq!(func.outputs[0].name, "Directory");
        assert_eq!(func.events.len(), 1);
        assert_eq!(func.events[0].name, "Changed");
        // Impure so it re-executes (and re-emits the passthrough) on every fire.
        assert!(matches!(func.behavior, FuncBehavior::Impure));
    }

    #[test]
    fn classifies_filesystem_event_kinds() {
        use super::is_content_change;
        use notify::EventKind;
        use notify::event::{
            AccessKind, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind, RenameMode,
        };

        // Subscribed: writes, new files, removes, renames.
        assert!(is_content_change(&EventKind::Create(CreateKind::File)));
        assert!(is_content_change(&EventKind::Remove(RemoveKind::File)));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Data(
            DataChange::Content
        ))));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Name(
            RenameMode::Both
        ))));
        // Coarse `Modify(Any)` is kept — macOS FSEvents reports real writes that way.
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Any)));

        // Dropped: metadata-only changes (incl. access time), reads, and the
        // uncategorized catch-alls.
        assert!(!is_content_change(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::AccessTime)
        )));
        assert!(!is_content_change(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::Permissions)
        )));
        assert!(!is_content_change(&EventKind::Access(AccessKind::Read)));
        assert!(!is_content_change(&EventKind::Any));
        assert!(!is_content_change(&EventKind::Other));
    }

    #[tokio::test]
    async fn passes_directory_through_and_seeds_watcher() {
        let dir = unique_temp_dir();
        let dir_str = dir.to_str().unwrap();
        let lib = fs_watch_library();
        let func = lib.by_name("Watch Directory").unwrap();

        let mut ctx = ContextManager::default();
        let mut state = AnyState::default();
        let event_state = SharedAnyState::default();

        let out = invoke_watch(
            &func.lambda,
            &mut ctx,
            &mut state,
            &event_state,
            dir_str,
            true,
        )
        .await;
        assert_eq!(out.as_fs_path(), Some(dir_str));

        let guard = event_state.lock().await;
        let ws = guard.get::<WatchState>().expect("watcher seeded");
        assert_eq!(ws.path, dir_str);
        assert!(ws.recursive);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reuses_watcher_until_params_change() {
        let dir = unique_temp_dir();
        let dir_str = dir.to_str().unwrap();
        let lib = fs_watch_library();
        let func = lib.by_name("Watch Directory").unwrap();

        let mut ctx = ContextManager::default();
        let mut state = AnyState::default();
        let event_state = SharedAnyState::default();

        invoke_watch(
            &func.lambda,
            &mut ctx,
            &mut state,
            &event_state,
            dir_str,
            true,
        )
        .await;
        let sig1 = stored_signal(&event_state).await.unwrap();

        // Same params on re-run: the watcher must be kept, not rebuilt.
        invoke_watch(
            &func.lambda,
            &mut ctx,
            &mut state,
            &event_state,
            dir_str,
            true,
        )
        .await;
        let sig2 = stored_signal(&event_state).await.unwrap();
        assert!(
            Arc::ptr_eq(&sig1, &sig2),
            "unchanged params must reuse watcher"
        );

        // Flipping `recursive` must rebuild the watcher with the new mode.
        invoke_watch(
            &func.lambda,
            &mut ctx,
            &mut state,
            &event_state,
            dir_str,
            false,
        )
        .await;
        let sig3 = stored_signal(&event_state).await.unwrap();
        assert!(
            !Arc::ptr_eq(&sig2, &sig3),
            "changed params must rebuild watcher"
        );

        let guard = event_state.lock().await;
        assert!(!guard.get::<WatchState>().unwrap().recursive);
        drop(guard);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_path_skips_watcher_and_event_parks() {
        let lib = fs_watch_library();
        let func = lib.by_name("Watch Directory").unwrap();

        let mut ctx = ContextManager::default();
        let mut state = AnyState::default();
        let event_state = SharedAnyState::default();

        let out = invoke_watch(&func.lambda, &mut ctx, &mut state, &event_state, "", true).await;
        assert_eq!(out.as_fs_path(), Some(""));
        assert!(event_state.lock().await.get::<WatchState>().is_none());

        // Without a watcher the `changed` event must park forever, not panic.
        let fired = timeout(
            Duration::from_millis(200),
            func.events[0].event_lambda.invoke(event_state.clone()),
        )
        .await;
        assert!(fired.is_err(), "event must not fire without a watcher");
    }

    /// Clearing the path tears the previous watcher down — otherwise the old
    /// directory's OS watch stays installed and keeps firing `Changed` while
    /// the node outputs an empty path.
    #[tokio::test]
    async fn clearing_path_tears_down_previous_watcher() {
        let dir = unique_temp_dir();
        let dir_str = dir.to_str().unwrap();
        let lib = fs_watch_library();
        let func = lib.by_name("Watch Directory").unwrap();

        let mut ctx = ContextManager::default();
        let mut state = AnyState::default();
        let event_state = SharedAnyState::default();

        invoke_watch(
            &func.lambda,
            &mut ctx,
            &mut state,
            &event_state,
            dir_str,
            true,
        )
        .await;
        assert!(event_state.lock().await.get::<WatchState>().is_some());

        invoke_watch(&func.lambda, &mut ctx, &mut state, &event_state, "", true).await;
        assert!(
            event_state.lock().await.get::<WatchState>().is_none(),
            "the stale watcher must be dropped with its OS watch"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn watcher_signals_on_content_change() {
        let dir = unique_temp_dir();
        let ws = WatchState::new(dir.to_str().unwrap(), false, Duration::ZERO).unwrap();
        let signal = ws.signal.clone();

        // Absorb any spurious event from creating the directory itself, so the
        // assertion below measures the file write specifically.
        let _ = timeout(Duration::from_millis(300), signal.notified()).await;

        std::fs::write(dir.join("new.txt"), b"hello").unwrap();

        timeout(Duration::from_secs(5), signal.notified())
            .await
            .expect("creating a file in the watched dir must fire the watcher");

        drop(ws);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn debounce_collapses_burst_into_one_fire() {
        let dir = unique_temp_dir();
        let lib = fs_watch_library();
        let func = lib.by_name("Watch Directory").unwrap();

        // Seed per-node state with a 200ms-debounce watcher, then drive the real
        // `changed` event lambda against a hand-pulsed signal (a "burst").
        let event_state = SharedAnyState::default();
        let ws = WatchState::new(dir.to_str().unwrap(), false, Duration::from_millis(200)).unwrap();
        let signal = ws.signal.clone();
        event_state.lock().await.set(ws);

        let lambda = func.events[0].event_lambda.clone();
        let es = event_state.clone();
        let start = Instant::now();
        let handle = tokio::spawn(async move { lambda.invoke(es).await });
        tokio::task::yield_now().await;

        // Two pulses 50ms apart — both inside the 200ms window.
        signal.notify_one();
        sleep(Duration::from_millis(50)).await;
        signal.notify_one();

        // The first pulse must not have fired immediately — it's being debounced.
        assert!(!handle.is_finished(), "must not fire mid-burst");

        // Exactly one fire, only after the window elapses past the last pulse.
        timeout(Duration::from_secs(2), handle)
            .await
            .expect("debounced fire")
            .unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(200),
            "fire must be delayed by the debounce window, got {:?}",
            start.elapsed()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
