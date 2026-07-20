# Common Architecture Review

Status: 2026-07-20

## Executive summary

`common` has a sound cross-crate core: serialization formats, cancellation,
typed IDs, introspection, spans, file discovery, and small numerical helpers
all have multiple legitimate consumers or define workspace-wide contracts.
The previous review's critical introspection work, fallible/sorted file
discovery, extension-policy split, and `KeyIndexVec` removal are reflected in
the current code and are no longer active findings. Ordered editor placement
state now uses `IndexMap` with its custom serialization beside the owning
document module (`darkroom/src/core/document/mod.rs:180`,
`darkroom/src/core/document/serde.rs:1-49`).

The remaining work is concentrated in two areas:

1. Return Scenarium- and Lumos-specific primitives to their sole production
   owners, reducing `common`'s dependency and API surface.
2. Remove avoidable copies and unconditional proc-macro build cost from the
   serialization/introspection layer.

The LZ4 size contract, authored-file publication, and persistent frame-cache
identity findings are reflected in the current code and are no longer active.

## Review scope

This review covers every production module under `common/src`, the
`common-derive` proc macro, both manifests, and production call sites needed to
establish ownership and persistence behavior. Tests, benches, examples, and
test-support code were excluded from findings.

| Disposition | Current modules |
| --- | --- |
| Keep in `common` | serialization and `SerdeFormat`, `CancelToken`, introspection traits/value types, typed-ID macro, `Span`, file discovery, `FloatExt`, constants/debug helpers |
| Move to Scenarium | `Shared`, `Slot`, `PauseGate`, `ReadyState` |
| Move to Lumos | `BitBuffer2`, `Vec2us`, `Rgb`, CPU detection, bounded parallel mapping, `FnvHasher`, `SharedFn` |

## Batch 1 — Critical: keep serialization pairs symmetric

- [x] **Reject LZ4 values during encode when the paired decoder would reject them.** Encoding and decoding share the same 1 GiB ceiling, the encoder validates the JSON byte length before allocating compression output, and the 32-bit header uses a checked conversion. Boundary tests cover the ceiling, ceiling plus one, and `u32::MAX + 1` without multi-gigabyte fixtures; the ordinary LZ4 round trip still verifies the wire format.

## Batch 2 — High: make persisted state durable and collision-resistant

- [x] **Publish authored serialized state through one atomic replacement helper.** `common::file_utils` now publishes through uniquely named, exclusively created sibling files with explicit durable and cache modes. Durable publication synchronizes the file before replacement and the containing directory afterward on Unix; Windows uses replace-existing, write-through `MoveFileExW`. Document/graph export, library saves, and preferences use durable publication. CFA/calibration masters, Scenarium blobs, Lumos planes/sidecars, the document-cache `.gitignore`, and the TCP discovery token use cache publication. Tests cover replacement, concurrent writers, cleanup after write and replacement failures, and preservation of the prior file across both failure stages.

- [x] **Replace `FnvHasher` as persistent frame-cache identity before relocating it.** Persistent frame names now use the full BLAKE3 path digest. The source sidecar stores and compares the canonical path bytes, byte length, and signed nanoseconds since the Unix epoch; it is published last as the commit record after atomic plane and statistics publication. The loader also rejects a source whose identity changes during decode. FNV remains only in the transient star-filter spatial hash. Tests cover normal reuse, a forced cache-name collision between distinct sources, same-size rewrites within one second, and length changes with an unchanged timestamp.

## Batch 3 — Medium: put runtime synchronization in Scenarium

- [ ] **Move `Shared`, `Slot`, `PauseGate`, and `ReadyState` to their sole production owner and remove the resulting dependencies from `common`.** `Shared` only backs Scenarium's `SharedAnyState` (`common/src/shared.rs:5-28`, `scenarium/src/runtime/shared_any_state.rs:3-36`); `Slot` only serves the FPS event state (`scenarium/src/elements/worker_events_library.rs:44-55`, `scenarium/src/elements/worker_events_library.rs:76-105`); and `PauseGate`/`ReadyState` only coordinate Scenarium's worker event loop (`scenarium/src/worker/mod.rs:101-105`, `scenarium/src/worker/event_loop.rs:23-57`). Their presence makes every `common` consumer receive Tokio and arc-swap (`common/Cargo.toml:20`, `common/Cargo.toml:27`). Move the modules and tests into Scenarium, remove their root exports (`common/src/lib.rs:49-55`), and then remove unused dependencies from `common`. Validate Scenarium independently and use `cargo tree` to confirm unrelated `common` consumers no longer receive these runtime dependencies through this crate.

- [ ] **Make `PauseGate` close guards composable while moving it.** Each `close()` writes the same boolean (`common/src/pause_gate.rs:49-57`), and dropping any guard clears that boolean and wakes every waiter (`common/src/pause_gate.rs:61-70`). Two overlapping guards therefore reopen the gate when the first drops even though the second remains alive. Replace the flag with a close count and notify only on the `1 -> 0` transition, or enforce unique close ownership in the type. Verify two guards dropped in both orders and guards created from separate clones. The worker currently serializes its one guard (`scenarium/src/worker/mod.rs:179-195`), so this hardens the primitive's promised public behavior rather than fixing a currently observed worker race.

- [ ] **Reduce `Slot` to the send/observe contract Scenarium actually uses.** Production publishes with `send`, observes with `peek`, and waits with `peek_async` (`scenarium/src/elements/worker_events_library.rs:49-55`, `scenarium/src/elements/worker_events_library.rs:83-105`). `SlotError`, `take`, `take_async`, `try_take`, and `has_value` add a second ownership-transfer protocol in which a live peek makes taking fail (`common/src/slot.rs:6-8`, `common/src/slot.rs:47-88`, `common/src/slot.rs:109-115`), but no production caller uses it. Delete that protocol during relocation and keep a latest-value observable slot. Verify overwrite behavior, immediate observation, and wake-up of every pending observer.

## Batch 4 — Medium: return image-processing implementation to Lumos

- [ ] **Move `BitBuffer2`, `Vec2us`, and `Rgb` into coherent Lumos modules.** These public types (`common/src/lib.rs:37`, `common/src/lib.rs:51`, `common/src/lib.rs:57`) have no production consumer outside Lumos. `BitBuffer2` is used by star-detection/background masks (`lumos/src/stacking/star_detection/threshold_mask/mod.rs:25`, `lumos/src/background_mesh/workspace.rs:4`), `Vec2us` by image geometry and registration (`lumos/src/io/astro_image/mod.rs:18`, `lumos/src/stacking/registration/interpolation/mod.rs:15`), and `Rgb` by image operations (`lumos/src/image_ops/mod.rs:32`, `lumos/src/image_ops/stretching/mod.rs:27`). Relocate each type beside its domain owner, narrow visibility where possible, remove the root exports, and remove `aligned-vec` from `common/Cargo.toml:17`. Validate Lumos independently, including its SIMD and serialization paths.

- [ ] **Fix `BitBuffer2`'s empty iteration, dimension arithmetic, and hot assertions as part of the move.** Either zero dimension makes a buffer empty (`common/src/bit_buffer2.rs:126-136`), but `BitIter::next` checks only height before reading `(0, 0)` (`common/src/bit_buffer2.rs:324-345`), so iterating or converting a `0 x N` buffer panics. Construction also multiplies dimensions/stride without checked overflow (`common/src/bit_buffer2.rs:39-43`, `common/src/bit_buffer2.rs:65-79`), and per-bit access pays release assertions (`common/src/bit_buffer2.rs:138-178`) despite being used in pixel loops. Track one logical index or terminate on either zero dimension, validate allocation sizes with checked arithmetic at construction, and use `debug_assert!` for per-bit internal bounds while retaining cold input validation. Verify `0 x N`, `N x 0`, `0 x 0`, exact iterator lengths/conversions, overflow rejection, debug out-of-bounds behavior, and scalar/SIMD agreement.

- [ ] **Move or remove the remaining Lumos-only utilities.** CPU dispatch and bounded parallel mapping are production-used only by Lumos (`lumos/src/math/sum/mod.rs:29-70`, `lumos/src/stacking/pipeline/streaming.rs:6-86`), while `SharedFn` only wraps the `ProgressCallback` alias (`common/src/shared_fn.rs:3-39`, `lumos/src/stacking/progress.rs:25-35`). Move the CPU and fallible batching helpers into Lumos, reconciling their shape with Imaginarium's standalone detector where practical (`imaginarium/src/cpu_features.rs:1-55`); retain the infallible batching variant only if a production caller remains. Replace `SharedFn` with a domain-local optional `Arc<dyn Fn...>`, move any transient FNV use after the persistent-cache change, and remove obsolete modules/exports. This should remove Rayon from `common/Cargo.toml:19` and substantially narrow its utility surface. Validate bounded concurrency, input-order preservation, error propagation, SIMD dispatch on supported architectures, and progress callback behavior.

## Batch 5 — Low: remove avoidable copies and build dependencies

- [ ] **Give slice deserialization a direct slice dispatch instead of routing through `Read`.** `deserialize(&[u8], ..)` wraps the slice in `Cursor`, after which TOML and Bitcode copy the entire payload into scratch with `read_to_end` (`common/src/serde.rs:118-143`). Darkroom's undo stack already holds a contiguous range but deliberately calls the reader path, adding the same O(payload) copy on every decode (`darkroom/src/core/edit/action_stack/mod.rs:260-278`). Dispatch slice inputs directly to each backend's slice decoder and reserve `deserialize_from` for real readers; update undo to use the slice entry point. Verify identical decoding and trailing-data behavior for every format and confirm repeated undo/redo does not grow or fill an O(payload) decode scratch buffer.

- [ ] **Replace the exported normalization trait with a private consuming helper.** `NormalizeString` is publicly re-exported (`common/src/lib.rs:48`) but its only production use is internal TOML serialization (`common/src/serde.rs:6-7`, `common/src/serde.rs:95-97`). That caller already owns the `String`, yet an already normalized value is cloned (`common/src/normalize_string.rs:32-40`). Replace the trait with a private `fn normalize(String) -> String`: append a missing newline in place and allocate only when removing CR/CRLF. Verify empty, LF, CRLF, lone-CR, and missing-final-newline cases, plus capacity reuse on the no-CR path.

- [ ] **Make the introspection derive opt-in and narrow its parser features.** `common-derive` is an unconditional dependency (`common/Cargo.toml:15-16`) and is always re-exported (`common/src/introspect/mod.rs:299`), although Lens is the only production derive consumer (`lens/src/astro/configs.rs:11-50`, `lens/src/astro/configs.rs:196-209`). The workspace also enables Syn's `full` parser (`Cargo.toml:72`) while this macro only parses derive inputs, fields, types, and attributes. Put the proc-macro dependency/re-export behind an `introspect-derive` feature enabled by Lens, and give `common-derive` only the Syn features it uses rather than inheriting `full`. Verify the introspection traits/value types remain usable without the feature, all Lens derives still expand, and `cargo tree` for a non-Lens consumer no longer contains `common-derive` through `common`.

## Open questions

- [ ] **Is `CancelToken::reset` guaranteed to run only after every clone from the previous operation has quiesced?** Live clones share one flag, and `reset` clears it for all of them (`common/src/cancel_token.rs:12-22`, `common/src/cancel_token.rs:46-64`). The worker awaits its run before reset (`scenarium/src/worker/mod.rs:179-195`), which is safe only if every spawned/blocking task has joined. If quiescence is a hard invariant, narrow reset ownership and document it at the worker boundary; if operations can overlap, create a fresh token per operation so an old clone remains cancelled permanently.
