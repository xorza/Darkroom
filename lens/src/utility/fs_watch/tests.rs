use crate::utility::fs_watch::{WATCH_DIRECTORY_FUNC_ID, WatchState, fs_watch_library};
use scenarium::StaticValue;
use scenarium::{AnyState, ContextManager, FuncBehavior, FuncLambda};
use scenarium::{DynamicValue, InvokeError, InvokeInput, OutputDemand, SharedAnyState};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;
use tokio::time::{Duration, sleep, timeout};

fn unique_temp_dir() -> std::path::PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("lens-watch-test-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn try_invoke_watch(
    lambda: &FuncLambda,
    ctx: &mut ContextManager,
    state: &mut AnyState,
    event_state: &SharedAnyState,
    path: &str,
    recursive: bool,
) -> Result<DynamicValue, InvokeError> {
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
    let demand = [OutputDemand::Produce];
    let mut outputs = [DynamicValue::Unbound];
    lambda
        .invoke(ctx, state, event_state, &mut inputs, &demand, &mut outputs)
        .await?;
    Ok(outputs[0].clone())
}

async fn invoke_watch(
    lambda: &FuncLambda,
    ctx: &mut ContextManager,
    state: &mut AnyState,
    event_state: &SharedAnyState,
    path: &str,
    recursive: bool,
) -> DynamicValue {
    try_invoke_watch(lambda, ctx, state, event_state, path, recursive)
        .await
        .unwrap()
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
    use crate::utility::fs_watch::is_content_change;
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
async fn invalid_replacement_drops_previous_watcher() {
    let dir = unique_temp_dir();
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
        dir.to_str().unwrap(),
        true,
    )
    .await;

    let file = dir.join("not-a-directory.txt");
    std::fs::write(&file, b"content").unwrap();
    let error = try_invoke_watch(
        &func.lambda,
        &mut ctx,
        &mut state,
        &event_state,
        file.to_str().unwrap(),
        false,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("watch path is not a directory"));
    assert!(event_state.lock().await.get::<WatchState>().is_none());

    invoke_watch(
        &func.lambda,
        &mut ctx,
        &mut state,
        &event_state,
        dir.to_str().unwrap(),
        true,
    )
    .await;
    let missing = dir.join("missing");
    let error = try_invoke_watch(
        &func.lambda,
        &mut ctx,
        &mut state,
        &event_state,
        missing.to_str().unwrap(),
        false,
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("failed to inspect watch directory")
    );
    assert!(event_state.lock().await.get::<WatchState>().is_none());

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
