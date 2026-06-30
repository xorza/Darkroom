use std::path::Path;
use std::sync::Arc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::Notify;

use crate::data::{DataType, FsPathConfig, FsPathMode, StaticValue};
use crate::event_lambda::EventLambda;
use crate::func_lambda::FuncLambda;
use crate::function::{Func, FuncInput};
use crate::library::Library;
use crate::prelude::FuncId;

pub const WATCH_DIRECTORY_FUNC_ID: FuncId = FuncId::from_u128(0x1318c24c2ac74a9aa454281bdbdc4ffc);

/// Per-node state shared between the func lambda (which builds the OS watcher)
/// and the `changed` event lambda (which awaits the next notification).
struct WatchState {
    path: String,
    recursive: bool,
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
    fn new(path: &str, recursive: bool) -> notify::Result<Self> {
        let signal = Arc::new(Notify::new());
        let callback_signal = signal.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
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
            .finish_non_exhaustive()
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
        Func::new(WATCH_DIRECTORY_FUNC_ID, "watch directory")
            .category("Filesystem")
            .description(
                "Passes a directory through unchanged and fires `changed` when its contents change.",
            )
            .input(FuncInput::required("directory", directory_type()))
            .input(FuncInput::required("recursive", DataType::Bool).default(true))
            .output("directory", directory_type())
            .event(
                "changed",
                EventLambda::new(|state| {
                    Box::pin(async move {
                        let signal =
                            state.lock().await.get::<WatchState>().map(|w| w.signal.clone());
                        match signal {
                            Some(signal) => signal.notified().await,
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

                        if !path.is_empty() {
                            let mut guard = event_state.lock().await;
                            let stale = guard
                                .get::<WatchState>()
                                .map(|w| w.path != path || w.recursive != recursive)
                                .unwrap_or(true);
                            if stale {
                                let watch_state = WatchState::new(&path, recursive)
                                    .map_err(anyhow::Error::from)?;
                                guard.set(watch_state);
                            }
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
    use crate::context::ContextManager;
    use crate::data::{DynamicValue, StaticValue};
    use crate::func_lambda::{FuncLambda, InvokeInput, OutputUsage};
    use crate::function::FuncBehavior;
    use crate::prelude::{AnyState, SharedAnyState};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::time::{Duration, timeout};

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
        let inputs = [
            InvokeInput {
                value: StaticValue::FsPath(path.to_string()).into(),
            },
            InvokeInput {
                value: StaticValue::Bool(recursive).into(),
            },
        ];
        let usage = [OutputUsage::Needed(1)];
        let mut outputs = [DynamicValue::Unbound];
        lambda
            .invoke(ctx, state, event_state, &inputs, &usage, &mut outputs)
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
        let func = lib.by_name("watch directory").expect("func registered");

        assert_eq!(func.id, WATCH_DIRECTORY_FUNC_ID);
        assert_eq!(func.inputs.len(), 2);
        assert_eq!(func.inputs[0].name, "directory");
        assert_eq!(func.inputs[1].name, "recursive");
        assert_eq!(func.inputs[1].default_value, Some(StaticValue::Bool(true)));
        assert_eq!(func.outputs.len(), 1);
        assert_eq!(func.outputs[0].name, "directory");
        assert_eq!(func.events.len(), 1);
        assert_eq!(func.events[0].name, "changed");
        // Impure so it re-executes (and re-emits the passthrough) on every fire.
        assert!(matches!(func.behavior, FuncBehavior::Impure));
    }

    #[tokio::test]
    async fn passes_directory_through_and_seeds_watcher() {
        let dir = unique_temp_dir();
        let dir_str = dir.to_str().unwrap();
        let lib = fs_watch_library();
        let func = lib.by_name("watch directory").unwrap();

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
        let func = lib.by_name("watch directory").unwrap();

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
        let func = lib.by_name("watch directory").unwrap();

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

    #[tokio::test]
    async fn watcher_signals_on_content_change() {
        let dir = unique_temp_dir();
        let ws = WatchState::new(dir.to_str().unwrap(), false).unwrap();
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
}
