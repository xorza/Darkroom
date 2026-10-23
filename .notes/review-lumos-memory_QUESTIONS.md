# Questions: lumos memory plan

## Batch 8, raster decode bound: where may the imaginarium change be pushed?

The fix needs `Image::decode_admitted` in imaginarium, a submodule with its own repository. The
darkroom branch must point at an imaginarium commit the remote holds, and the work may push only
to its own branch.

| Option | What it does | Cost |
|---|---|---|
| **A branch of the same name in imaginarium (recommended)** | Push `lumos-memory-reuse` to imaginarium, and point darkroom's branch at it. | A second branch to merge, in imaginarium, before darkroom's. |
| Commit to imaginarium `main` | Push the change straight to imaginarium's `main`. | Lands in imaginarium before darkroom's branch is reviewed. |
| Bound the decode in lumos alone | Read the TIFF header in lumos before imaginarium decodes it. | A new dependency on `tiff` in lumos, and the file opened twice. |

Blocked: batch 8 (`imaginarium/src/image/io.rs`, `lumos/src/io/raster.rs`), and batch 9's
`io/raster.rs:107` item: moving an `L_F32` plane out of an `Image` needs a by-value conversion in
imaginarium, which has only `TryFrom<&Image>`.
