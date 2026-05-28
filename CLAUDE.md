AI coding rules for Rust projects.

## Project state

Scenarium is a Cargo workspace for a node-based data processing pipeline framework with a visual editor. Workspace members: `common` (shared utilities, buffers, file formats), `scenarium` (the node-graph framework: graph, execution, functions), `darkroom` (the editor app, the default-member), `lens` (image-processing funclib), `lumos` (astronomical image processing), `imaginarium` (image library with wgpu GPU ops), `quickbench` (in-test benchmarking utilities), and `palantir` (our GUI library). `default-members = ["darkroom"]`; only `.tmp` is `exclude`d. `imaginarium`, `quickbench`, and `palantir` are git submodules (see `.gitmodules`). The frozen `darkroom-egui-deprecared/` crate is **not** a workspace member.

**GUI rewrite in progress.** The editor is mid-migration off egui:

- **`darkroom-egui-deprecared/`** — the old egui-based editor. Frozen, kept only as a reference for porting features. Do not add new functionality here. Bug-fix only if something blocks the rewrite. The directory name is intentionally misspelled (`-deprecared`); keep using it verbatim.
- **`darkroom/`** — the new editor, being rewritten on top of **Palantir** (the in-development immediate-mode GUI library in `palantir/`). This is where new editor work goes.
- **`palantir/`** — our own Rust GUI library, also under active development. A workspace member (and git submodule) with its own conventions in `palantir/CLAUDE.md` and `palantir/DESIGN.md`. Treat it as a sibling project: changes to `darkroom/` may require coordinated changes in `palantir/`.

Both `darkroom/` and `palantir/` are pre-1.0 and break freely. The egui UI conventions section below applies **only** to `darkroom-egui-deprecared/`.
