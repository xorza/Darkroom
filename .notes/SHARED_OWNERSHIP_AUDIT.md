# Shared ownership audit

This audit examines each use of `Rc`, `Arc`, `Weak`, `RefCell`, `Cell`, `Mutex`, `RwLock`,
`OnceLock`, `OnceCell`, `LazyLock`, the atomics, `thread_local!`, the `tokio::sync` primitives
and the channels in the workspace. It also includes the submodules `palantir` and `fits-well`.
For each use, it answers three questions:

1. Is shared ownership necessary? Can one owner with `&` or `&mut`, a move, a scoped thread, a
   rayon reduce, a channel message or an index into an owning store replace it?
2. If sharing is necessary, is the primitive the standard one?
3. Is the use robust? The audit looks for races, lost wakes, deadlocks, `RefCell` panics,
   `Rc` cycles and missing bounds.

This is a temporary proposal. When a change lands, its lasting decision moves into the
`AGENTS.md` or the doc comment of the system that it governs. Then delete the change from this
file. The confirmed bugs are also in `.notes/ISSUES.md`, `palantir/.notes/ISSUES.md` and
`fits-well/.notes/ISSUES.md`.

## Inventory

Counts of the declarations, from `rg` on 2026-11-05. The counts include tests.

| Crate | `Rc` | `Arc` | `RefCell` | `Cell` | `Mutex` | `Weak` | `Once*`/`Lazy*` | Atomics | `thread_local!` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| common | 0 | 1 | 0 | 1 | 2 | 0 | 1 | 33 | 2 |
| scenarium | 0 | 54 | 0 | 0 | 11 | 0 | 1 | 31 | 0 |
| imaginarium | 0 | 0 | 0 | 0 | 0 | 0 | 4 | 2 | 0 |
| libraw | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 |
| lumos | 0 | 12 | 0 | 3 | 11 | 0 | 5 | 45 | 1 |
| lens | 0 | 8 | 0 | 0 | 1 | 0 | 2 | 0 | 0 |
| darkroom | 0 | 33 | 2 | 2 | 3 | 0 | 0 | 8 | 2 |
| app | 0 | 4 | 0 | 0 | 2 | 0 | 1 | 0 | 0 |
| palantir | 88 | 35 | 34 | 37 | 4 | 3 | 3 | 42 | 2 |
| fits-well | 3 | 4 | 3 | 8 | 1 | 0 | 2 | 10 | 1 |

`simd` holds none. No crate uses `RwLock`, `ArcSwap`, `parking_lot`, `crossbeam` or `static mut`.

## Summary

- **One open bug:** the editor's report channel has no bound. The scenarium command channel,
  the `palantir` `WakeLatch` bug and the four test and admission bugs are fixed.
- **Production shared ownership is mostly correct.** Most `Arc`s carry immutable artifacts
  between the host and the worker. Most `Rc`s in `palantir` come from one deliberate decision:
  each window's `Ui` holds a clone of the host-wide `UiResources`.
- **About 5 uses can go**, and **about 19 can change** to a simpler or more standard primitive.
  Every `palantir` and `fits-well` item and the poisoning policy are done.

## Recommended order

1. Fix bug 1 with a mailbox that merges reports at push.
2. Replace the atomic union-find in `lumos`.
3. Replace `Arc<ScenariumLibrary>` with an owned `Library`.
4. Do the other simplifications in the order of this document.

## Decisions taken

- **Poisoning: a poisoned lock is a logic error.** `common::MutexExt` and `common::CondvarExt`
  panic on poison in one place, and `clippy.toml` refuses the direct calls; the rule is in
  `AGENTS.md`. Refused: recovering with `PoisonError::into_inner`, which lets a panic in a thread
  no one joins leave broken data that a test then passes over; `parking_lot`, a new direct
  dependency whose guard turns `Send` for the whole build if any dependency enables
  `send_guard`.
- **Bug 7: check the prefix again.** On a failure of frame k, run `SetFacts::check_set` over
  frames `0..=k` and report its first failure. Refused: frame 0 alone first, about +20% wall time
  for each run; no early check, which decodes a whole mismatched set before it fails.
- **Bug 2: merging mailbox.** A std `Mutex<BatchIntent>` with a tokio `Notify` (the tokio
  `Notify` docs' channel), the send folding into the intent. Refused: a bounded channel with a
  busy error, which the GUI must handle for edits the worker merges anyway; `watch`, whose
  receiver cannot reset the merged state.
- **Bug 1: merge at push.** The bridge's mailbox merges a run's reports into the run-ending one
  and carries the count of runs ended, with a test that the merged fold equals the whole fold.
  Refused: pausing event loops while hidden, which also stops a Watch Directory; a shared
  `RunState`, which is the opposite of this audit.

## Confirmed bugs

### 1. The editor's worker report channel has no bound

- **Where:** `darkroom/src/core/worker_bridge/mod.rs:50` (`channel::<WorkerReport>()`), drained
  by `App::update`.
- **Failure:** `palantir/src/host/winit/window/mod.rs:347` skips the frame, both update and
  record, while the window is minimized or occluded. The editor does not pause the worker in
  that state. A graph with a live event loop, for example scenarium's Frame Event at 60 Hz
  (`worker_events_library/mod.rs:89`) or a busy Watch Directory, posts Activity reports
  (`worker_task/mod.rs:336,373`) and a run summary for each run. These reports collect for as
  long as the window stays hidden. Headless MCP grows in the same way between tool calls,
  because no frame runs then.
- **Contrast:** `PreviewSink` keeps the latest value only, so it stays bounded on the same path.
- **Recommended fix:** a mailbox that merges reports, in the shape of `PreviewSink`. It keeps the
  latest Activity, the latest summary and a count of the runs that ended. `RunState` counts
  `runs_ended`, and the driver waits on that count. Only Installed, Cleared and Error stay
  FIFO. The commands bound their count.
- **Refused:** a std `sync_channel`. A full channel blocks a tokio worker thread inside the
  callback.

## Shared ownership to remove

### lumos: atomic union-find in star labeling

- **Where:** `lumos/src/star_detection/labeling/union_find.rs` (the whole file),
  `labeler.rs:104-122` and `run.rs:157` (`RunJoin.union_find: &UnionFind`).
- **Why it is safe:** the parallel pass unions labels only inside one strip.
  `Strip::label_rows` calls `merge_with_prev` on its own rows. The only union across strips is
  `stitch`, which runs on one thread after the join. Thus, the CAS loops never contend.
- **Cost today:** one shared `next_label.fetch_add` per run, on one cache line that each worker
  touches, and false sharing on interleaved parent slots.
- **Replacement:** count the runs of each strip (`count_runs` already counts each row). Do a
  prefix sum of those counts to get the label bases. Give each run its base plus its index.
  Split the parent `Vec<u32>` into `&mut` slices, one for each strip, with a plain union-find in
  each. `stitch` and `build_label_map` then run on `&mut [u32]`.
- **Gain:** no atomics or orderings to examine, and provisional labels in raster order on any
  schedule.
- **Effort:** medium, about three files.

### darkroom: `Arc<ScenariumLibrary>` in `RuntimeLibrary`

- **Where:** `darkroom/src/core/runtime_library.rs:17`, cloned at `gui/app/mod.rs:694`.
- **Replacement:** an owned `Library`, with `cur() -> &Library`.
- **Why it is safe:** the library never leaves the main thread. `Compiler::compile` takes
  `&Library` (`scenarium/src/exec/compile/mod.rs:121`), and `AppCtx` holds `&'a Library`. The
  clone for each frame exists only to avoid a borrow conflict. `ctx` is consumed by
  `self.session.frame(...)`, which borrows fields disjoint from `self.runtime`. `ctx` is dead
  before `run_command(&mut self)` can recompose.
- **Gain:** the borrow checker enforces the doc comment's promise of "one registry per frame".
  Today, the `Arc` enforces it.
- **Sites to change:** `gui/graph_ctx/internals.rs:33`, `gui/app/session/internals.rs:49`,
  `session/bench.rs:115` (keep the `RuntimeLibrary` alive and borrow from it),
  `session/tests.rs:57` and `App::library()` at `app/mod.rs:764`.
- **Effort:** small to medium, about 8 sites. Deref coercion does most of the change from
  `&Arc` to `&Library`.

### darkroom: `RefCell<FullImage>` and the `Jobs` clone for each image

- **Where:** `darkroom/src/gui/state/preview_store/mod.rs:116,125`, `DrawablePreview::full(&self)`
  at `:395`, and `job_queue.rs:26,70-85`.
- **Why they exist:** `full(&self)` mutates from the record pass, which holds `RunState`
  immutably. Each image holds a sender, so that it can queue its own full-resolution job.
- **Replacement:** the usual immediate-mode route. The viewer
  (`pane/image_viewer/mod.rs:139`) pushes a "full raster wanted (node, ticket)" request through
  the frame's `Requests`. `RunState` or `PreviewStore` applies it with `&mut` in the next
  `update`.
- **What goes:** the `RefCell`, the `Clone` and sender count in `job_queue.rs`, and the `borrow()`
  in `in_flight()`. The queue then has one sender, so when `Jobs` drops, the inbox ends.
- **Cost:** the full conversion starts one frame later. The thumbnail at the same texels already
  shows during that frame.
- **Note:** there is no re-entry path today, so no double-borrow panic can occur. This is a
  cleanup, not a fix.
- **Effort:** medium.

### lumos: `AtomicFlagCounts` and `fallback_pixels`

- **Where:** `lumos/src/image/flags/flag_counts.rs:94-107`. Its only user is
  `combine/frame_cache/mod.rs:350-352,573-575`, which also holds the `AtomicU64`
  `fallback_pixels`.
- **Replacement:** change the row loop's `for_each` into
  `.map(|row| tallies).reduce(Default::default, merged)` with the existing
  `FlagCounts::merged`. Fold the result of each chunk into locals of the `FnMut` chunk callback.
- **Gain:** one type less, and 17 fewer atomic read-modify-writes for each row. The sums are
  already deterministic, so the gain is less shared state.
- **Effort:** small.

### lumos: `deblended_components`

- **Where:** `lumos/src/star_detection/star_detector/stages/detect/mod.rs:145` (`AtomicUsize`).
- **Replacement:** `par::chunks(..).enumerate().map(|..| count).sum()`.
- **Effort:** trivial.

## Primitives to simplify or make standard

### lumos: `ScratchPool` used as scratch for each worker

- **Sites:**
  - `combine/frame_cache/mod.rs:442` (a pool made for one call)
  - `bg_mesh/mod.rs:123,139`
  - `registration/resample/mod.rs:221,230`
  - `star_detection/detection_filter/mod.rs:127`
  - `image/flags/pixel_flags.rs:261,266`
  - `drizzle/accumulator/output_band.rs:134,352`
- **From → to:** an `Arc<Mutex<Vec>>` lease for each item → `common::PerWorker::get()`.
- **Why:** each item takes the shared lock two times and does an `Arc` increment and decrement on
  the same two cache lines. `PerWorker` gives each worker its own slot, aligned to 128 bytes.
- **Two ways to do one thing:** `scratch_pool/mod.rs:9-12` says that `PerWorker` is for scratch
  that lives for one call. But `deblend/deblenders.rs:18` and `calibration/cosmic_ray/mono.rs:78`
  keep a `PerWorker` on an owner across runs. Thus, the split that the doc states does not exist.
- **Keep `ScratchPool`** for planes held across stages: the `DetectionResources` floats, bitmasks
  and surfaces, and the `Pooled` fields in `prepared_frame`, `bg_estimate` and `sky_noise`.
- **Caveats:** threads outside the pool share one `PerWorker` slot (the blocking lock in
  `per_worker.rs:62`), so the owner must belong to one slot thread. The memory accounting changes
  from `workers` to `workers + 1` (`heap_bytes` in `resources.rs`).

### app: channels in the MCP server

- **`session_thread.rs:76` job queue:** change the unbounded std `mpsc` to
  `tokio::sync::mpsc::channel(N)`. Use `send().await` in `run()`, `blocking_send` in `close()` and
  `blocking_recv` on the editor thread. The tokio docs give this as the bounded bridge between
  sync and async code. Jobs run one at a time, and everything must have a bound.
- **`session_thread.rs:77` (`started`) and `mcp/mod.rs:63` (`sessions`):** both are unbounded
  channels that carry exactly one message. Use `tokio::sync::oneshot` (already imported) with
  `blocking_recv`, or `sync_channel(1)`, so that the bound is visible.

### app: `Session { driver: Option<Driver>, ended: Option<Ended> }`

- **Where:** `app/src/mcp/session_thread.rs:49-50`.
- **Change:** an enum `Live(Driver) | Ended(Ended)`. The type allows `(None, None)`, which
  `unreachable!` at `:191` rejects. The doc line at `:215-216` occurs two times.

### darkroom: `native_open` flag

- **Where:** `darkroom/src/gui/dialogs.rs:26,133,138`, created at `lib.rs:74-75`, read in
  `driver/window_link.rs:32,38`.
- **Placement:** move `Arc<AtomicBool>` into `DialogFallback::Native(Arc<AtomicBool>)`. The flag
  means nothing under `Refuse`, which still allocates one. The caller creates the flag and passes
  it in, and the `native_open()` getter goes.
- **Ordering:** Release and Acquire at `dialogs.rs:230,232` and `window_link.rs:32,38` can be
  `Relaxed`. The flag guards no other memory (*Rust Atomics and Locks*, chapter 3, stop flag).
- **Stuck flag:** if `native()` panics, the flag stays raised. This matters only under the
  unwinding `mcp` profile. A drop guard closes it.
- **Stale doc:** the `WindowLink` doc at `window_link.rs:12` speaks of "the flags", but there is
  only one flag.

### darkroom: `Arc::clone` of the compiled graph in `RunState`

- **Where:** `darkroom/src/gui/state/run_state/mod.rs:354`.
- **Change:** a plain `as_ref()` borrow. Each later write touches a disjoint field, and
  `drop_empty_nodes()` runs after the last use. `apply_node_progress` at `:340` already borrows.

### darkroom: `Arc<[DragStart]>`

- **Where:** `gui/pane/graph_ui/gesture/group_drag/mod.rs:51,148` and
  `core/edit/graph_intent/mod.rs:109`.
- **Change:** `Rc<[DragStart]>`. The editor is already `!Send` (`session_thread.rs:26-27`), and
  the intent stays on the main thread. The value is low: one atomic increment for each drag frame.

### darkroom: `Wake` wraps `palantir::Waker`

- **Where:** `darkroom/src/core/wake.rs:9` (`Arc<dyn Fn()>`), built at `gui/app/mod.rs:146`.
- **Problem:** `palantir::Waker` is already an `Arc<dyn Fn>` and is `Clone + Send + Sync`, so
  this is a double indirection.
- **Change:** use `palantir::Waker` directly. This needs a waker with no host in palantir for
  `wake::internals::noop`, because `Waker::new` is `pub(crate)`. The value is low.

### scenarium: smaller items

- **`graph/func/event.rs:10,29`:** each event fire clones `SharedAnyState` and boxes a new future.
  At frame rate, that is one allocation each frame. The structural fix is an event lambda that
  returns one future for the life of the loop, so the box is allocated once for each loop start.
  The priority is low, until events become hot.

### lumos and common: smaller items

- **`common/src/par/mod.rs:140,146`:** `lowest_failure` uses `Acquire` and `AcqRel` but publishes
  no data. The outcomes are read after the scope joins. Use `Relaxed`, as `CancelToken` and
  `first_failure.rs` do for stop hints.
- **`lumos/src/frame_store/file_set.rs:35`:** `first: Mutex<Option<Box<First>>>` and
  `KeptSet.probes: Vec<Mutex<Probed>>` are two take-once cells for one index. Fold `First` into
  the cell of each path, as a `Probed::Early` variant with the vec always present. Then each path
  has one lock. The value is small.
- **`lumos/src/frame_store/decode_cache/mod.rs:88-97`:** `keep_gain` holds the `Mutex` across
  `publish_gain`, a file write. This is correct, because it makes the check-then-write occur
  exactly once. But each frame with a gain waits behind the first write. This is acceptable with
  few gains. Examine it again if the number of gains increases.
- **`common/src/fs/tests.rs:112`:** the test uses `Arc<Barrier>` with `thread::spawn`. A scoped
  thread that borrows `&Barrier` is sufficient.

## Correct uses

Each item below was checked against its call sites. Keep these uses as they are.

### scenarium

- **Immutable artifacts shared between host and worker:** `Arc<CompiledGraph>` in
  `protocol.rs:17,44`, `batch_intent/mod.rs:15`, `engine/mod.rs:41` and `worker_task`. The host
  keeps the program that it sent, and `Installed` tells which one landed.
- **`Arc<RunSummary>`** with the `Arc::get_mut` reuse in the publisher (`run_summary/mod.rs:30-56`).
  It avoids the deep clone of `make_mut`.
- **Copy-on-write registry:** `Library.codecs: Arc<Codecs>` with `Arc::make_mut`
  (`library/mod.rs:33,141`), copied into `CompiledGraph.codecs`. `Arc<dyn CustomValueCodec>` in
  `TypeEntry` and `Codecs` is necessary for the identity check `same_as` with `Arc::ptr_eq`.
- **Shared immutable closures:** `FuncLambda(Arc<AsyncLambda>)` and `EventLambda(Arc<AsyncEvent>)`.
- **Values:** `DynamicValue::Custom(Arc<dyn CustomValue>)`, with a move on the last use and
  `try_unwrap` in `into_custom`. `decode` returns an `Arc`, which avoids a reallocation from `Box`
  to `Arc`. `RunError::Invoke(Arc<InvokeError>)`, because `InvokeError` holds a
  `Box<dyn Error>` and the error is cloned into status rows. `DataType::FsPath(Arc<FsPathConfig>)`.
- **`SharedAnyState` sharing itself:** the node lambda and its event task both touch it. Only the
  primitive changes. Reconcile replaces it only after the loop stops.
- **Worker synchronization:** `PauseGate` over `watch::Sender<usize>`. `wait_for` reads the value
  again under the lock, so no wake is lost. The event channel is bounded at
  `EVENT_LOOP_BACKPRESSURE`. `recv_many` and `join_next_with_id` are cancel-safe inside
  `select!`. The shutdown `CancellationToken` beside the `CancelToken` lets shutdown preempt
  queued messages. `path_identities.rs` moves its job to `run_blocking` and back.
- **Cancel and `reset()`:** `worker_task/mod.rs:200` resets `run_cancel` after `recv_many`. A
  cancel in that window clears, which agrees with the documented rule. `Relaxed` is correct.
- **Tests:** `Arc<Mutex<Vec<_>>>` recorders, `Arc<AtomicBool>` and `Arc<AtomicUsize>` flags, and
  `Mutex<bool>` with `Condvar` in `replacement.rs`.

### darkroom, app and lens

- **Cross-thread mailboxes:** `PreviewSink` `Mutex<HashMap>` (`core/preview_sink/mod.rs:36`) keeps
  the latest value and is held only for the swap. `Arc<PreviewSink>` is necessary for a
  `'static`, `Send + Sync` lambda. `LogRing` `Arc<Mutex<VecDeque>>` has many writers and a cap of
  200.
- **Preview converter:** `job_queue.rs` `Mutex<Queue>` with `Condvar`. The wait loop checks again
  under the lock, so no wake is lost. A plain channel cannot replace a job for a node in place.
  The result channel at `preview_converter.rs:105` has no bound, but queued jobs bound it in
  practice.
- **Worker:** `Arc<CompiledGraph>` is shared with the worker thread. The bridge's `Drop` joins the
  worker task through `block_on(exit())`.
- **lens:** `Arc<tokio::sync::Notify>` in `fs_watch` (`:38`). The stored permit of `notify_one`
  makes the `timeout` debounce cancel-safe. `KeptModel` `Arc<Mutex<T>>` (`ml.rs:155`) keeps a share
  across a cancelled or failed blocking run. `Arc<[FieldKind]>` (`config_node.rs:92`) is shared by
  `Send` lambdas.
- **Forced globals:** the `thread_local!` `Cell`s in `alloc_audit.rs` (global allocator,
  `#[cfg(test)]`). The `thread_local!` `RefCell<Option<Sink>>` in `platform/macos/open_files.rs:41`
  (a bare-fn IMP with no user data). `PANICS` `OnceLock<Mutex<HashMap<ThreadId, …>>>` in
  `session_thread.rs:21` (the process-wide panic hook). Each carries its `expect`.
- **MCP ownership:** `Arc<SessionThread>` (`mcp/mod.rs:48,71`, `darkroom_server.rs:39`), because
  rmcp needs a `Clone` handler. `Joined` drops the sender before it joins. In `serve()`, the
  `abort` and then `await` of the watch task has no race.
- **Not found:** `Arc` cycles, a `RefCell` double-borrow path, a std `Mutex` held across an
  `.await` and a lock-order hazard.

### common, lumos, imaginarium and libraw

- **Cancellation flags:** `CancelToken` (`Arc<AtomicBool>`, `Relaxed`) and the libraw
  `Hooks.corrupt` (`Relaxed`, read after `libraw_unpack` returns). `omp_set_num_threads` sets the
  setting of the calling thread only.
- **Tallies read after a join:** `CalibrationNotes` and `FrameParking.parked`, with `Relaxed` and
  the happens-before of the join.
- **Process-unique name counters:** `fs/mod.rs:320`, `temp_dir.rs:45` and `RunScratch.next`, each
  with a `create_new` retry.
- **`StageCounter` with `SeqCst`:** necessary. It is a store-then-load handoff on two locations,
  which acquire and release cannot order.
- **Take-once cells in parallel jobs:** `par::slots_owned` with `Mutex<Option<T>>`, and the
  `FileSet` probes. Std has no take-once cell.
- **`PerWorker` slots:** a `Mutex` in each slot with `try_lock`, aligned to the cache line. This
  is the standard shape when `unsafe` is denied.
- **Shared values:** `FrameGain::Shared(Arc<FlatGain>)`, `SharedGains`, `prepared_flat`,
  `ProgressCallback` (`Arc<dyn Fn + Send + Sync>`) and the `Arc` of `ScratchPool` for leases held
  across stages.
- **`OnceLock` on owned state:** `FlatGain.key`, `FrameAdmission.first_facts` (except bug 7) and
  `Gpu.kernels`.
- **Static tables of pure functions:** `LanczosLut::lut()` and `PixelQuadrature::gauss_legendre`.
  They cannot be `const`, because they use `sin` and Newton's method.
- **Forced `thread_local!`:** `common` `WorkerPool::TOP_LEVEL` (rayon runs the body, and no owner
  can carry the mark) and `lumos` `live_heap` `ROLE` (global allocator).
- **Test fixtures:** `lena`, `test_gpu`, `temp_root`, the memory-budget `POOL`, the bench
  `OnceCell` fixtures, the `Cell` call counters and the `Arc<Mutex<Vec>>` progress sinks.

### palantir

- **Per-window `Ui` sharing host resources:** `UiResources` holds `TextShaper`, `IconRegistry`,
  `ImageRegistry`, `Rc<SharedDevice>`, `Rc<AppSettings>` `Cell`s, `WindowDirectory`, `Clipboard`
  and `FamilyKeys`. This is necessary while each `WindowDriver` owns a `Ui` that `Harness::ui()`
  and `OffscreenHost::ui()` show outside frames. The alternative with one owner is a `Ui<'a>` for
  each frame that borrows `&mut` resources, as masonry and xilem do. That change goes through each
  widget signature and into darkroom. egui makes the same choice (`Context` is
  `Arc<RwLock<ContextImpl>>`). `Cell` for small `Copy` settings and `RefCell` for maps are the
  correct primitives.
- **RAII resource handles:** `ImageHandle` `Rc<ImageToken>` with its shared generation `Cell`,
  `IconSet` `Rc<IconSetToken>` with a `Weak` back reference, and `Rc<WgpuImageStore>`. A handle
  that the user drops at any location must reach the store, as wgpu's own handles do.
  `Rc<IconTable>` identity is the idempotency key.
- **Public `TextShaper`** (`Rc<Shared{RefCell, Cell}>`): builders accept a shaper that the user
  builds. The epoch `Cell` beside the `RefCell` keeps frames where all lookups hit away from the
  borrow. A `glyphs()` borrow held across a `Ui` text call panics. That is documented misuse, and
  no internal path does it.
- **Copy-on-write `Rc<Theme>`** (`theme_pair`, `window_theme`, `theme_transition` with `make_mut`
  and `get_mut`): the same pattern as egui's `Arc<Style>`. Known cost: an app that keeps a clone
  of `ui.theme()` after the frame causes a 28 KiB clone each frame during a transition.
- **`GpuView` `Rc<RefCell<T: GpuPaint>>` and `GpuPaintRef`:** a deliberate public design in
  `palantir/AGENTS.md`. It is borrowed for one draw at a time. egui and iced keep user GPU state
  in a type map that the renderer owns (`CallbackResources`, `shader::Storage`). That is an
  alternative, not a defect. `GpuPaintRef::eq` uses `Rc::ptr_eq` on `dyn`, which ignores the
  vtable since Rust 1.72.
- **`Arc` that a thread or a `Send` wgpu callback forces:** `Waker`, `WakeLatch`, `WakeSignal` (`Mutex<bool>` with `Condvar`), `Arc<FrameLog>`, `MapState`
  `Arc<AtomicU32>` (AcqRel CAS on a generation tag), `DeviceHealth`, the `done` atomics of both
  tracers, `Remote` `given_up`, `Gpu::pipeline_turn` `Arc<Mutex<()>>` (wgpu#10547),
  `Arc<WinitWindow>` (wgpu `Surface<'static>`) and `Arc<dyn InstanceDisplay>`.
- **wgpu 30.0.1 handles:** `Device`, `Queue` and `Adapter` are `Clone`. No handle is in an `Arc`
  without a reason. The `Arc<wgpu::Adapter>` in `test_gpu` is necessary, because its `Weak` tells
  when the last device is gone.
- **Process-wide atomics:** `TextureId` and `TextEpoch` `IdCounter` (`Relaxed` `fetch_add`,
  unique in the process so that a handle on a different host misses and does not alias), the Tracy
  `CLAIMED` counter and the bench `BETWEEN_CASES`.
- **Forced statics:** the `panic_probe` `thread_local!` (the panic hook takes no user data) and
  the `test_gpu` static `Mutex<Registry>` with `Weak<Adapter>` and `Weak<OpenDevice>`.
- **Lazy caches and test counters:** `Backends` `OnceCell` devices and `TestOnly<Cell<u32>>`.
- **Not found:** `Rc` cycles, `SeqCst`, and a re-entrant `RefCell` borrow. The checked paths are
  icon `drain_released`, `WgpuImageStore::read()`, `GpuPaint::init` and `paint` before the glyph
  lease, and `probe_text` with `&mut self`.

### fits-well

- `Arc<FitsError>` sources in `hdus/mod.rs:24` and `column/mod.rs:166`: a non-`Clone` error that
  occurs again as a source.
- `reader/source.rs` `Rc<RefCell<Fetches>>`: a test reads it after the cursor moves into the
  reader.
- `counting.rs` `COUNTED` and `CALLS` `thread_local!`: forced by the global allocator.
- The `map_init` scratch of each rayon job: never more alive than workers, as measured on a
  4096² image in up to 4096 tiles. The `init` calls grow with the tiles, at most one in 4 to 5
  of them on 10 workers. Refused: `with_min_len`, which limits rayon's balancing for no memory
  gain, and a slot for each worker, which brings a `Mutex` back. The `map_tiles` doc holds the
  measurement.

## Sources

- Mara Bos, *Rust Atomics and Locks*, chapters 2 and 3: stop flags, release and acquire,
  happens-before.
- tokio docs, `tokio::sync::Mutex`: the std `Mutex` is preferred unless the guard is held across
  an `.await`. `tokio::sync::mpsc`: the bounded bridge between sync and async code.
  `tokio::sync::Notify`: the stored permit.
- std docs: `LazyLock`, `OnceLock`, `std::thread::scope`, `MutexGuard` is `!Send`, trait upcasting
  (Rust 1.86).
- rayon docs: `find_map_first`, `map_init`, `reduce`.
- egui `Context` and `Arc<Style>`, iced `shader::Storage`, masonry and xilem contexts.
- wgpu 30 changelog: `Device`, `Queue` and `Adapter` are `Clone`, and render passes do not borrow
  resources.
