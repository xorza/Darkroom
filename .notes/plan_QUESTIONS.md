# Questions from executing `plan.md`

Each entry names the plan item it blocks, the options, a recommendation, and what waits on the
answer.

## Q1. Where a float TIFF's dimensions are read from its header (B9)

**Finding.** `LinearImage::peek` reads no TIFF header, so the registered pipeline decodes the first
float-TIFF light in full to size the run (`pipeline/light_source.rs`, `LightFiles::shape`), before
the plan exists, and the intake decodes it again. The cost is one extra decode of one light and a
transient one frame wide outside the plan; FITS lights, the common case, peek their header already.
`imaginarium` has no header-only read: `Image::read_file` decodes the pixels. The plan put the probe
in `imaginarium`, which is a submodule with its own repository, and this run commits only to its
own branch of this repository.

| Option | What it does | Cost |
|---|---|---|
| **A. A probe in `imaginarium`** (recommended) | `imaginarium` gains a header-only read of an image's description (size, channel count, sample type) through the `tiff` crate it already uses; `LinearImage::peek` calls it for TIFF | A commit in the `imaginarium` repository and a submodule bump here, which you make or allow |
| B. A TIFF header reader in `lumos` | `lumos` reads the first IFD's width, height, samples per pixel and sample format itself, with no new dependency | A second TIFF parser whose acceptance must agree with the decoder's, bit for bit, and BigTIFF, byte order and multi-page files to handle twice |
| C. Leave it | The first TIFF light keeps its double decode | One decode of one light per run, and its transient outside the plan |

**Recommendation.** A: the header belongs to the library that decodes the file, so the probe and
the decode cannot disagree.

**Blocks.** B9.
