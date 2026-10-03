use common::TempDir;

use crate::fs_watch::{WATCH_DIRECTORY_FUNC_ID, WatchState, fs_watch_library};
use scenarium::internals::func_invoker::FuncInvoker;
use scenarium::{ConstValue, DynamicValue, Func, FuncBehavior, InvokeError, SharedAnyState};
use std::fs;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task;
use tokio::time::{Duration, Instant, sleep, timeout};

/// One Watch Directory node, called again and again the way a node runs across
/// graph runs: its state persists between calls.
#[derive(Debug)]
struct WatchNode {
    func: Func,
    invoker: FuncInvoker,
}

impl WatchNode {
    fn new() -> WatchNode {
        WatchNode {
            func: fs_watch_library()
                .by_name("Watch Directory")
                .unwrap()
                .clone(),
            invoker: FuncInvoker::default(),
        }
    }

    async fn try_call(
        &mut self,
        path: &str,
        recursive: bool,
        debounce_ms: i64,
    ) -> Result<DynamicValue, InvokeError> {
        let inputs = [
            ConstValue::FsPath(path.to_string()).into(),
            ConstValue::Bool(recursive).into(),
            ConstValue::Int(debounce_ms).into(),
        ];
        let mut outputs = self.invoker.call(&self.func, inputs).await?;
        Ok(outputs.remove(0))
    }

    async fn call(&mut self, path: &str, recursive: bool) -> DynamicValue {
        self.try_call(path, recursive, 250).await.unwrap()
    }

    fn event_state(&self) -> SharedAnyState {
        self.invoker.event_state()
    }
}

async fn stored_signal(event_state: &SharedAnyState) -> Option<Arc<Notify>> {
    event_state
        .lock()
        .await
        .get::<WatchState>()
        .map(|w| Arc::clone(&w.signal))
}

#[test]
fn registers_watch_directory_func() {
    let lib = fs_watch_library();
    let func = lib.by_name("Watch Directory").expect("func registered");

    assert_eq!(func.id, WATCH_DIRECTORY_FUNC_ID);
    assert_eq!(func.inputs.len(), 3);
    assert_eq!(func.inputs[0].name, "Directory");
    assert_eq!(func.inputs[1].name, "Recursive");
    assert_eq!(func.inputs[1].default_value, Some(ConstValue::Bool(true)));
    assert_eq!(func.inputs[2].name, "Debounce (ms)");
    assert_eq!(func.inputs[2].default_value, Some(ConstValue::Int(1000)));
    assert_eq!(func.outputs.len(), 1);
    assert_eq!(func.outputs[0].name, "Directory");
    assert_eq!(func.events.len(), 1);
    assert_eq!(func.events[0].name, "Changed");
    // Impure so it re-executes (and re-emits the passthrough) on every fire.
    assert!(matches!(func.behavior, FuncBehavior::Impure));
}

#[test]
fn classifies_filesystem_event_kinds() {
    use crate::fs_watch::is_content_change;
    use notify::EventKind;
    use notify::event::{
        AccessKind, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind, RenameMode,
    };

    // Subscribed: writes, new files, removes, renames.
    assert!(is_content_change(EventKind::Create(CreateKind::File)));
    assert!(is_content_change(EventKind::Remove(RemoveKind::File)));
    assert!(is_content_change(EventKind::Modify(ModifyKind::Data(
        DataChange::Content
    ))));
    assert!(is_content_change(EventKind::Modify(ModifyKind::Name(
        RenameMode::Both
    ))));
    // Coarse `Modify(Any)` is kept — macOS FSEvents reports real writes that way.
    assert!(is_content_change(EventKind::Modify(ModifyKind::Any)));

    // Dropped: metadata-only changes (incl. access time), reads, and the
    // uncategorized catch-alls.
    assert!(!is_content_change(EventKind::Modify(ModifyKind::Metadata(
        MetadataKind::AccessTime
    ))));
    assert!(!is_content_change(EventKind::Modify(ModifyKind::Metadata(
        MetadataKind::Permissions
    ))));
    assert!(!is_content_change(EventKind::Access(AccessKind::Read)));
    assert!(!is_content_change(EventKind::Any));
    assert!(!is_content_change(EventKind::Other));
}

#[tokio::test]
async fn passes_directory_through_and_seeds_watcher() {
    let dir = TempDir::new("lens-watch");
    let dir_str = dir.path().to_str().unwrap();
    let mut node = WatchNode::new();
    let event_state = node.event_state();

    let out = node.call(dir_str, true).await;
    assert_eq!(out.as_fs_path(), Some(dir_str));

    let guard = event_state.lock().await;
    let ws = guard.get::<WatchState>().expect("watcher seeded");
    assert_eq!(ws.path, dir_str);
    assert!(ws.recursive);
}

#[tokio::test]
async fn reuses_watcher_until_params_change() {
    let dir = TempDir::new("lens-watch");
    let dir_str = dir.path().to_str().unwrap();
    let mut node = WatchNode::new();
    let event_state = node.event_state();

    node.call(dir_str, true).await;
    let sig1 = stored_signal(&event_state).await.unwrap();

    // Same params on re-run: the watcher must be kept, not rebuilt.
    node.call(dir_str, true).await;
    let sig2 = stored_signal(&event_state).await.unwrap();
    assert!(
        Arc::ptr_eq(&sig1, &sig2),
        "unchanged params must reuse watcher"
    );

    // Flipping `recursive` must rebuild the watcher with the new mode.
    node.call(dir_str, false).await;
    let sig3 = stored_signal(&event_state).await.unwrap();
    assert!(
        !Arc::ptr_eq(&sig2, &sig3),
        "changed params must rebuild watcher"
    );

    let guard = event_state.lock().await;
    assert!(!guard.get::<WatchState>().unwrap().recursive);
    drop(guard);
}

/// The debounce input reaches the watcher, and changing it alone retunes the
/// live watcher in place rather than rebuilding its OS watch. A negative
/// debounce is none.
#[tokio::test]
async fn debounce_input_retunes_the_live_watcher() {
    let dir = TempDir::new("lens-watch");
    let dir_str = dir.path().to_str().unwrap();
    let mut node = WatchNode::new();
    let event_state = node.event_state();
    let debounce = || async {
        let guard = event_state.lock().await;
        let watch = guard.get::<WatchState>().unwrap();
        (Arc::clone(&watch.signal), watch.debounce)
    };

    node.try_call(dir_str, true, 250).await.unwrap();
    let (first, at_250) = debounce().await;
    assert_eq!(at_250, Duration::from_millis(250));

    node.try_call(dir_str, true, 40).await.unwrap();
    let (second, at_40) = debounce().await;
    assert_eq!(at_40, Duration::from_millis(40));
    assert!(
        Arc::ptr_eq(&first, &second),
        "a debounce change keeps the watcher"
    );

    node.try_call(dir_str, true, -5).await.unwrap();
    assert_eq!(debounce().await.1, Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn empty_path_skips_watcher_and_event_parks() {
    let mut node = WatchNode::new();
    let event_state = node.event_state();

    let out = node.call("", true).await;
    assert_eq!(out.as_fs_path(), Some(""));
    assert!(event_state.lock().await.get::<WatchState>().is_none());

    // Without a watcher the `changed` event must park forever, not panic.
    let fired = timeout(
        Duration::from_millis(200),
        node.func.events[0].event_lambda.invoke(event_state.clone()),
    )
    .await;
    assert!(fired.is_err(), "event must not fire without a watcher");
}

/// Clearing the path tears the previous watcher down — otherwise the old
/// directory's OS watch stays installed and keeps firing `Changed` while
/// the node outputs an empty path.
#[tokio::test]
async fn clearing_path_tears_down_previous_watcher() {
    let dir = TempDir::new("lens-watch");
    let dir_str = dir.path().to_str().unwrap();
    let mut node = WatchNode::new();
    let event_state = node.event_state();

    node.call(dir_str, true).await;
    assert!(event_state.lock().await.get::<WatchState>().is_some());

    node.call("", true).await;
    assert!(
        event_state.lock().await.get::<WatchState>().is_none(),
        "the stale watcher must be dropped with its OS watch"
    );
}

#[tokio::test]
async fn invalid_replacement_drops_previous_watcher() {
    let dir = TempDir::new("lens-watch");
    let mut node = WatchNode::new();
    let event_state = node.event_state();

    node.call(dir.path().to_str().unwrap(), true).await;

    let file = dir.join("not-a-directory.txt");
    fs::write(&file, b"content").unwrap();
    let error = node
        .try_call(file.to_str().unwrap(), false, 250)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("watch path is not a directory"));
    assert!(event_state.lock().await.get::<WatchState>().is_none());

    node.call(dir.path().to_str().unwrap(), true).await;
    let missing = dir.join("missing");
    let error = node
        .try_call(missing.to_str().unwrap(), false, 250)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("failed to inspect watch directory")
    );
    assert!(event_state.lock().await.get::<WatchState>().is_none());
}

#[tokio::test]
async fn watcher_signals_on_content_change() {
    let dir = TempDir::new("lens-watch");
    let ws = WatchState::new(dir.path().to_str().unwrap(), false, Duration::ZERO).unwrap();
    let signal = Arc::clone(&ws.signal);

    // Absorb any spurious event from creating the directory itself, so the
    // assertion below measures the file write specifically.
    let _ = timeout(Duration::from_millis(300), signal.notified()).await;

    fs::write(dir.join("new.txt"), b"hello").unwrap();

    timeout(Duration::from_secs(5), signal.notified())
        .await
        .expect("creating a file in the watched dir must fire the watcher");

    drop(ws);
}

#[tokio::test(start_paused = true)]
async fn debounce_collapses_burst_into_one_fire() {
    let dir = TempDir::new("lens-watch");
    let lib = fs_watch_library();
    let func = lib.by_name("Watch Directory").unwrap();

    // Seed per-node state with a 200ms-debounce watcher, then drive the real
    // `changed` event lambda against a hand-pulsed signal (a "burst").
    let event_state = SharedAnyState::default();
    let ws = WatchState::new(
        dir.path().to_str().unwrap(),
        false,
        Duration::from_millis(200),
    )
    .unwrap();
    let signal = Arc::clone(&ws.signal);
    event_state.lock().await.set(ws);

    let lambda = func.events[0].event_lambda.clone();
    let es = event_state.clone();
    let start = Instant::now();
    let handle = tokio::spawn(async move { lambda.invoke(es).await });
    task::yield_now().await;

    // Two pulses 50ms apart, both inside the 200ms window: the window restarts
    // at the second pulse, so the one fire lands at 50 + 200 = 250ms.
    signal.notify_one();
    sleep(Duration::from_millis(50)).await;
    signal.notify_one();
    sleep(Duration::from_millis(199)).await;
    assert!(!handle.is_finished(), "must not fire inside the window");

    handle.await.unwrap();
    assert_eq!(start.elapsed(), Duration::from_millis(250));
}
