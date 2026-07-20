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

The remaining implementation work is concentrated in the
serialization/introspection layer: removing avoidable copies and unconditional
proc-macro build cost.

The LZ4 size contract, authored-file publication, and persistent frame-cache
identity findings are reflected in the current code and are no longer active.
Lumos-specific image primitives and execution helpers now live with their
production owner, leaving `common` focused on cross-crate contracts.

## Review scope

This review covers every production module under `common/src`, the
`common-derive` proc macro, both manifests, and production call sites needed to
establish ownership and persistence behavior. Tests, benches, examples, and
test-support code were excluded from findings.

| Disposition | Current modules |
| --- | --- |
| Keep in `common` | serialization and `SerdeFormat`, `CancelToken`, introspection traits/value types, typed-ID macro, `Span`, file discovery/publication, `FloatExt`, constants/debug helpers |

## Batch 1 — Critical: keep serialization pairs symmetric

- [x] **Reject LZ4 values during encode when the paired decoder would reject them.** Encoding and decoding share the same 1 GiB ceiling, the encoder validates the JSON byte length before allocating compression output, and the 32-bit header uses a checked conversion. Boundary tests cover the ceiling, ceiling plus one, and `u32::MAX + 1` without multi-gigabyte fixtures; the ordinary LZ4 round trip still verifies the wire format.

## Batch 2 — High: make persisted state durable and collision-resistant

- [x] **Publish authored serialized state through one atomic replacement helper.** `common::file_utils` now publishes through uniquely named, exclusively created sibling files with explicit durable and cache modes. Durable publication synchronizes the file before replacement and the containing directory afterward on Unix; Windows uses replace-existing, write-through `MoveFileExW`. Document/graph export, library saves, and preferences use durable publication. CFA/calibration masters, Scenarium blobs, Lumos planes/sidecars, the document-cache `.gitignore`, and the TCP discovery token use cache publication. Tests cover replacement, concurrent writers, cleanup after write and replacement failures, and preservation of the prior file across both failure stages.

- [x] **Replace `FnvHasher` as persistent frame-cache identity before relocating it.** Persistent frame names now use the full BLAKE3 path digest. The source sidecar stores and compares the canonical path bytes, byte length, and signed nanoseconds since the Unix epoch; it is published last as the commit record after atomic plane and statistics publication. The loader also rejects a source whose identity changes during decode. FNV remains only in the transient star-filter spatial hash. Tests cover normal reuse, a forced cache-name collision between distinct sources, same-size rewrites within one second, and length changes with an unchanged timestamp.

## Batch 3 — Medium: put runtime synchronization in Scenarium

- [x] **Move Scenarium-owned runtime synchronization out of `common`.** `SharedAnyState` now owns `Arc<tokio::sync::Mutex<AnyState>>` directly, event-loop startup uses Tokio's `Barrier`, and `PauseGate` lives under Scenarium's worker. The former `Shared` and `ReadyState` wrappers, Common's root exports, and Common's Tokio dependency are deleted.

- [x] **Make `PauseGate` close guards composable while moving it.** A Tokio `watch<usize>` now holds the close count, and guard drops notify waiters only when the count reaches zero. Tests cover both drop orders, guards created from separate clones, repeated close/reopen cycles, and the boundary where a waiter has already passed.

- [x] **Remove `Slot` in favor of the existing per-node state synchronization.** Scenarium's FPS event source now stores `FpsEventState` directly in `SharedAnyState`: the event lambda clones a snapshot under its existing Tokio mutex before sleeping, and the node lambda mutates the state under the same lock. The custom `Slot`, `SlotError`, ownership-transfer protocol, root export, and Common's `arc-swap` dependency are deleted. Existing worker tests verify repeated state updates and event-loop observation end to end.

## Batch 4 — Medium: return image-processing implementation to Lumos

- [x] **Move `BitBuffer2`, `Vec2us`, and `Rgb` into coherent Lumos modules.** `BitBuffer2` now lives beside Lumos's packed-mask algorithms, `Vec2us` under `math`, and `Rgb` under its sole production owner, `image_ops`. `BitBuffer2` and `Rgb` are crate-private; `Vec2us` remains public because `ImageDimensions` exposes it. Common's modules and root exports are deleted. The mask kernels need row padding but do not issue aligned loads, so `BitBuffer2` now uses `Vec<u64>` and `aligned-vec` was removed from both Common and Lumos rather than transferred.

- [x] **Fix `BitBuffer2`'s empty iteration, dimension arithmetic, and hot assertions as part of the move.** Layout construction returns an allocation-free empty layout when either dimension is zero, then checks row-stride, logical-dimension, and padded-storage overflow independently for non-empty buffers. Iteration tracks one logical index and terminates at the exact logical length. Per-bit internal bounds use `debug_assert!`; cold slice/layout validation remains unconditional. Tests cover ordinary and `usize::MAX` zero-dimension shapes, exact iteration/conversion order and length, every non-empty overflow stage, debug out-of-bounds access, padding exclusion, and the existing mask kernel comparisons.

- [x] **Move or remove the remaining Lumos-only utilities.** Lumos reuses Imaginarium's public cached CPU detector, extended with the SSE2 and combined AVX2+FMA predicates Lumos needs. The only production batching variant, fallible input-order-preserving `try_par_map_limited`, now lives in Lumos's `concurrency` module; the unused infallible variant is deleted. `ProgressCallback` owns its optional `Arc<dyn Fn...>` directly, and the transient star-filter grid uses the standard randomized `HashMap` because it has no stable-order or persistent-identity requirement. Common's CPU, parallel, shared-function, and FNV modules/exports are gone, along with its Rayon dependency. Tests cover the concurrency cap, order, early error boundary, empty input, invalid limits, CPU predicates, and progress delivery/default behavior.

## Batch 5 — Low: remove avoidable copies and build dependencies

- [ ] **Give slice deserialization a direct slice dispatch instead of routing through `Read`.** `deserialize(&[u8], ..)` wraps the slice in `Cursor`, after which TOML and Bitcode copy the entire payload into scratch with `read_to_end` (`common/src/serde.rs:118-143`). Darkroom's undo stack already holds a contiguous range but deliberately calls the reader path, adding the same O(payload) copy on every decode (`darkroom/src/core/edit/action_stack/mod.rs:260-278`). Dispatch slice inputs directly to each backend's slice decoder and reserve `deserialize_from` for real readers; update undo to use the slice entry point. Verify identical decoding and trailing-data behavior for every format and confirm repeated undo/redo does not grow or fill an O(payload) decode scratch buffer.

- [ ] **Replace the exported normalization trait with a private consuming helper.** `NormalizeString` is publicly re-exported (`common/src/lib.rs:48`) but its only production use is internal TOML serialization (`common/src/serde.rs:6-7`, `common/src/serde.rs:95-97`). That caller already owns the `String`, yet an already normalized value is cloned (`common/src/normalize_string.rs:32-40`). Replace the trait with a private `fn normalize(String) -> String`: append a missing newline in place and allocate only when removing CR/CRLF. Verify empty, LF, CRLF, lone-CR, and missing-final-newline cases, plus capacity reuse on the no-CR path.

- [ ] **Make the introspection derive opt-in and narrow its parser features.** `common-derive` is an unconditional dependency (`common/Cargo.toml:15-16`) and is always re-exported (`common/src/introspect/mod.rs:299`), although Lens is the only production derive consumer (`lens/src/astro/configs.rs:11-50`, `lens/src/astro/configs.rs:196-209`). The workspace also enables Syn's `full` parser (`Cargo.toml:72`) while this macro only parses derive inputs, fields, types, and attributes. Put the proc-macro dependency/re-export behind an `introspect-derive` feature enabled by Lens, and give `common-derive` only the Syn features it uses rather than inheriting `full`. Verify the introspection traits/value types remain usable without the feature, all Lens derives still expand, and `cargo tree` for a non-Lens consumer no longer contains `common-derive` through `common`.

## Open questions

- [ ] **Is `CancelToken::reset` guaranteed to run only after every clone from the previous operation has quiesced?** Live clones share one flag, and `reset` clears it for all of them (`common/src/cancel_token.rs:12-22`, `common/src/cancel_token.rs:46-64`). The worker awaits its run before reset (`scenarium/src/worker/mod.rs:179-195`), which is safe only if every spawned/blocking task has joined. If quiescence is a hard invariant, narrow reset ownership and document it at the worker boundary; if operations can overlap, create a fresh token per operation so an old clone remains cancelled permanently.
