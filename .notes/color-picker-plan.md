# Colour picker — code design and implementation plan

Target crate: `palantir`. Companion to the design proposal
(`https://claude.ai/code/artifact/4f9c10f2-7152-4ad2-9b75-23fc08e5ef0e`).

The proposal chose a mesh for the field. This plan replaces that with a
CPU-generated texture, which is exact per texel. Section 4 gives the measured
reason and the renderer change that path needs.

---

## 1. Decisions

| Question | Answer |
| --- | --- |
| Picking model | **Okhsv** by default. HSV is the one alternate. |
| Values above 1.0 | **No.** The picker stays inside the sRGB gamut. Alpha is 0..1. |
| Swatch row | Optional, three states. See §7. |
| Field paint | CPU texture, exact per texel, downsampled by a power of two. Default 4. |
| Numeric row | `DragValue` for every channel. `TextEdit` for the hex field. |
| Layout | The panel in the HTML proposal, unchanged. |

Okhsv also removes a problem the proposal carried. Okhsl holds a gamut fold
that a sampled representation smears. Okhsv puts the whole gamut edge on the
`v = 1` boundary, so the interior is smooth in every direction.

---

## 2. Layers

Two layers, and they do not know about each other.

1. **Colour maths** in `primitives::color`. Pure conversion. No widget, no
   `Ui`, no theme. A node graph can hold an `Okhsv` without a picker.
2. **Widgets** in `widgets`. Five builders, one theme slot, one shared
   texture helper.

One renderer change enables layer 2. Section 4.3 states it.

---

## 3. Colour maths

### 3.1 Files

```
primitives/color/
  mod.rs           # Color, ColorU8 — exists, unchanged
  okhsv.rs         # struct Okhsv
  hsv.rs           # struct Hsv
  color_coords.rs  # enum ColorCoords — the model-tagged triple the widgets drive
  color_model.rs   # enum ColorModel — the tag the builder API takes
```

### 3.2 `Okhsv`

```rust
/// Hue, saturation and value in Ottosson's Okhsv space. Every axis is 0..1,
/// and `h` wraps. `s = 1` is the sRGB gamut edge, so every triple in the unit
/// cube names a colour inside the gamut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Okhsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl Okhsv {
    pub fn to_color(self) -> Color;
    /// `fallback_hue` answers grey, which has no hue of its own.
    pub fn from_color(color: Color, fallback_hue: f32) -> Self;
}
```

Port Ottosson's reference: `oklab_to_linear_srgb`, `compute_max_saturation`,
`find_cusp`, `toe`, `toe_inv`, `to_ST`, and the forward and inverse Okhsv
maps. Keep every helper a private free function in this file.

Three rules the port must follow.

- **Clamp the output.** The forward map returns a channel just below zero at
  the gamut edge. The reference port measures −1/255 at hue 0.0812. Clamp to
  0..1 before the `Color` is built.
- **Guard grey.** `from_color` divides by the chroma. Below `1e-7` chroma it
  returns `fallback_hue` and `s = 0`.
- **Take alpha nowhere.** `to_color` returns an opaque colour. The picker owns
  alpha.

No `From` impls. `From<Okhsv> for Color` would have to live in `color/mod.rs`
under the crate's impl rule, and the inverse cannot be a `From` at all because
it takes the fallback hue. Two named methods keep both directions in one file.

### 3.3 `Hsv`

The same two methods over the classic space. HSV is defined on
**sRGB-encoded** components, so `to_color` builds through `Color::rgb`, not
through `Color::linear_rgb`. Getting this backwards is the single most likely
bug in the whole feature, so the test in §10 pins it against hand-computed
values.

### 3.4 `ColorCoords` and `ColorModel`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ColorModel {
    #[default]
    Okhsv,
    Hsv,
}

impl ColorModel {
    pub fn label(self) -> &'static str;            // "Okhsv" / "HSV"
    pub fn axis_labels(self) -> [&'static str; 3]; // ["H", "S", "V"]
}

/// The model-tagged triple the widgets drive. The tag is the discriminant, so
/// a coordinate can never be read against the wrong model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorCoords {
    Okhsv(Okhsv),
    Hsv(Hsv),
}

impl ColorCoords {
    pub fn new(model: ColorModel, color: Color, fallback_hue: f32) -> Self;
    pub fn model(self) -> ColorModel;
    pub fn to_color(self) -> Color;
    pub fn with_model(self, model: ColorModel) -> Self; // re-derives through Color

    pub fn hue(self) -> f32;
    pub fn sat(self) -> f32;
    pub fn val(self) -> f32;
    pub fn set_hue(&mut self, h: f32);
    pub fn set_sat(&mut self, s: f32);
    pub fn set_val(&mut self, v: f32);
}
```

Every widget drives the three axes through these six accessors. No widget
matches on the model.

`with_model` goes through `Color`, so a model switch keeps the colour and
moves the handles. Grey keeps its hue because the current hue is the fallback.

---

## 4. Paint

### 4.1 Why a texture

Measured worst error against the exact colour, in 8-bit sRGB units, over
twelve hues. The mesh figures assume `MeshVertex`, whose colour is 8-bit
**linear** — half a step near black is 6.1/255 after the sRGB encode, which is
a floor no grid can pass.

| Paint path | Worst error | Cost per rebuild |
| --- | --- | --- |
| 7-stop gradient | 73 / 255 | none |
| Mesh, 17 × 17 | 7.8 / 255 | 289 conversions |
| Mesh, 33 × 33 | 6.3 / 255 | 1089 conversions |
| **Texture, factor 4** | **3.9 / 255** | 4 819 conversions, 19 KB upload |
| Texture, factor 2 | 1.5 / 255 | 19 021 conversions, 76 KB upload |
| Texture, factor 1 | 0.5 / 255 | 75 553 conversions, 302 KB upload |

Figures are for a 208 × 160 logical field at display scale 1.5, so 312 × 240
physical pixels. Factor 1 is the sRGB 8-bit quantization floor. The texture
wins on two counts, not one: the error is smaller, and it lands at the gamut
corner instead of in the darks, because image textures are
`Rgba8UnormSrgb` and decode on sample.

Factor 4 is the default. Factor 2 costs four times the CPU for 2.4/255, which
is below what anyone can see on a picker field.

### 4.2 What each surface costs

| Surface | Rebuild trigger | Texels at factor 4 |
| --- | --- | --- |
| Field | model, hue, or size changed | 79 × 61 |
| Hue bar | model or size changed | 79 × 4 |
| Alpha bar | colour or size changed | 79 × 6 |

The hue bar almost never rebuilds. The alpha bar rebuilds on every drag frame
but costs about 470 conversions, and its conversion is a straight alpha ramp
over the checker, not an Okhsv solve.

The field is the one to bench: about 4 800 Okhsv solves per frame while a hue
drag is in flight.

### 4.3 The renderer change this needs — do it first

`Ui::register_image` mints a new `TextureId` per call and the backend creates
a texture and a bind group for it. A picker that re-registers every hue-drag
frame would create and destroy a texture per frame. It would also allocate,
because `Image::from_rgba8` takes an owned `Vec<u8>`.

Add an in-place refresh. Three files.

- `renderer/image_registry.rs` — a second queue beside `pending`:
  `refresh: Vec<RefreshEntry>` where `RefreshEntry { id, size, texels: Vec<u8> }`,
  plus `drain_refresh`, plus a small free list so the drained buffers come
  back instead of being reallocated. Add
  `ImageHandle::update(&self, texels: &[u8])`, which asserts the length
  against `self.size()` and copies into a recycled buffer.
- `renderer/backend/image_textures/mod.rs` — drain the refresh queue with
  `queue.write_texture` into the cached texture. The bind group and the
  texture stay. A size mismatch cannot reach here, because `update` asserts.
- `ui/mod.rs` — nothing. The handle already holds the registry.

This is useful past the picker: any live CPU-side surface wants it.

### 4.4 `ColorSurface`

One helper owns a CPU texture and its handle. `widgets/color_surface.rs`,
`pub(crate)`.

```rust
#[derive(Debug)]
pub(crate) struct ColorSurface {
    handle: Option<ImageHandle>,
    texels: Vec<u8>,
    size: UVec2,
    stamp: u64,
}

impl ColorSurface {
    /// Rebuild when the size or the stamp moved, then hand back the handle to
    /// paint with. `fill` writes `size.x * size.y * 4` sRGB-encoded bytes.
    pub(crate) fn ensure(
        &mut self,
        ui: &Ui,
        size: UVec2,
        stamp: u64,
        fill: impl FnOnce(&mut Vec<u8>, UVec2),
    ) -> &ImageHandle;
}
```

Rules the implementation owes.

- `texels` is `clear()`ed and refilled, never reallocated. Reserve exactly
  once per size change with `reserve_exact`.
- The caller hashes its own inputs into `stamp` with `common::hash::Hasher`.
  Each of the three surfaces hashes a different set, so no shared key struct
  carries fields two of them ignore.
- Write **sRGB-encoded** bytes through `Color::to_srgb_u8`. The default
  `From<Color> for ColorU8` is a linear quantize and would paint far too
  bright.
- Clamp the texture size against `ui.max_image_dimension()`, then `expect` the
  registration. A device whose texture cap is under 80 px cannot run the GUI
  at all.

Texel count: `ceil(logical * ui.display().n() / downsample)`, floored at 2.

The crate's `srgb_to_linear` is a cubic approximation with error under 0.002,
so the texture inherits about 0.5/255 from the round trip. That is the same
approximation every other colour in the crate carries.

### 4.5 Handles

Two rings, drawn with `Shape::circle(center, radius, width)`. Dark outside at
alpha 0.75, light inside at alpha 0.95. Both are fixed colours, not palette
colours: the handle sits on top of every colour the field can show, so a
handle taken from the palette disappears on half the field.

The bars take the same treatment as a vertical two-tone line.

---

## 5. Widgets

Five builders. Each records one node, each returns the response type its
neighbours already return.

```
widgets/
  color_surface.rs         # ColorSurface — shared by field and strip
  color_field/mod.rs       # ColorField
  color_strip/mod.rs       # ColorStrip
  color_swatch.rs          # ColorSwatch
  color_button/mod.rs      # ColorButton
  color_picker/mod.rs      # ColorPicker + PickerState + History
  theme/color_picker.rs    # ColorPickerTheme
```

### 5.1 `ColorField`

```rust
#[derive(Debug)]
pub struct ColorField<'a> {
    node: Node,
    coords: &'a mut ColorCoords,
    downsample: u32,
    style: Option<&'a ColorPickerTheme>,
}

impl<'a> ColorField<'a> {
    #[track_caller]
    pub fn new(coords: &'a mut ColorCoords) -> Self;
    /// Texel size divisor. Must be a power of two in 1..=16. Default 4.
    pub fn downsample(self, n: u32) -> Self;
    style_setter!('a, ColorPickerTheme, color_picker);
    pub fn show(self, ui: &mut Ui) -> ValueResponse<'_>;
}
```

- Sense `CLICK | DRAG`, focusable.
- Size comes from the theme (`field_width` × `field_height`), not from
  `Sizing::FILL`. A fixed size is known at record time, so the texture is
  correct on the first frame with no one-frame lag. `Configure::size` still
  overrides it; a `FILL` override falls back to last frame's arranged rect,
  which is the lag `Slider` already accepts.
- Pointer maps like `Slider`: press jumps, drag tracks, the release frame
  commits. `x → sat`, `1 - y → val`.
- The texture stamp hashes model, hue and texel size.

### 5.2 `ColorStrip`

```rust
#[derive(Debug)]
pub struct ColorStrip<'a> {
    node: Node,
    kind: StripKind<'a>,
    downsample: u32,
    style: Option<&'a ColorPickerTheme>,
}

#[derive(Debug)]
enum StripKind<'a> {
    Hue(&'a mut ColorCoords),
    Alpha(&'a mut Color),
}

impl<'a> ColorStrip<'a> {
    #[track_caller] pub fn hue(coords: &'a mut ColorCoords) -> Self;
    #[track_caller] pub fn alpha(color: &'a mut Color) -> Self;
    pub fn downsample(self, n: u32) -> Self;
    style_setter!('a, ColorPickerTheme, color_picker);
    pub fn show(self, ui: &mut Ui) -> ValueResponse<'_>;
}
```

The hue bar binds the whole `ColorCoords`, not a bare `f32`, because hue alone
does not say which model to paint.

The alpha bar binds the whole `Color`: it reads the three colour channels for
the ramp and writes `a`. The checker is **baked into the texture**, so the bar
is one image and needs no second shape and no gradient.

### 5.3 `ColorSwatch`

```rust
#[derive(Debug)]
pub struct ColorSwatch {
    node: Node,
    color: Color,
    style: Option<&'static ColorPickerTheme>, // see note
}

impl ColorSwatch {
    #[track_caller] pub fn new(color: Color) -> Self;
    pub fn show(self, ui: &mut Ui) -> Response<'_>;
}
```

A chip with the checker behind it when the colour is translucent. It writes
nothing, so it returns a plain `Response` and the caller reads `clicked()`.
Give it the same `'a` lifetime as the others so `style_setter!` applies
unchanged; the `'static` above is shorthand, not the signature.

The checker here is small and static, so it is two `chrome_leaf` rows rather
than a texture.

### 5.4 `ColorPicker`

```rust
#[derive(Debug)]
pub struct ColorPicker<'a> {
    node: Node,
    color: &'a mut Color,
    alpha: bool,
    model: Option<ColorModel>,   // None = the retained one
    swatches: Swatches<'a>,
    downsample: u32,
    style: Option<&'a ColorPickerTheme>,
}

#[derive(Debug)]
enum Swatches<'a> {
    Hidden,
    Owned,
    Given(&'a [Color]),
}

impl<'a> ColorPicker<'a> {
    #[track_caller] pub fn new(color: &'a mut Color) -> Self;
    pub fn alpha(self, on: bool) -> Self;
    pub fn model(self, m: ColorModel) -> Self;
    pub fn history(self, on: bool) -> Self;        // writes Swatches::Owned
    pub fn swatches(self, s: &'a [Color]) -> Self; // writes Swatches::Given
    pub fn downsample(self, n: u32) -> Self;
    style_setter!('a, ColorPickerTheme, color_picker);
    pub fn show(self, ui: &mut Ui) -> ValueResponse<'_>;
}
```

`history` and `swatches` write one field, so the last call wins and no
combination can conflict.

### 5.5 `ColorButton`

A `ColorSwatch`-styled trigger that opens a `Popup` holding a `ColorPicker`.
Open state lives in the response map keyed off the trigger id, exactly as
`ComboBox` does it. `Popup::below(anchor)` with `ClickOutside::Dismiss`.

---

## 6. State

```rust
#[derive(Debug, Default)]
struct PickerState {
    coords: ColorCoords,
    /// The colour this picker last wrote. An outside edit is any difference.
    written: Color,
    field: ColorSurface,
    hue_bar: ColorSurface,
    alpha_bar: ColorSurface,
    history: History,
}
```

Held with `ui.with_state::<PickerState, _>(id, ...)`, so it is evicted with
the subtree.

The frame rule, in order:

1. If `*color != state.written`, rebuild `coords` from `*color` with
   `coords.hue()` as the fallback hue. Otherwise keep `coords` untouched.
2. Run the field, the bars and the numeric row. Each writes into `coords` or
   into the alpha.
3. If anything moved, set `*color = coords.to_color().with_alpha(alpha)` and
   `state.written = *color`.

Step 1 is why black keeps its hue. A picker that re-derives its axes every
frame loses the hue at `v = 0` and the handle jumps when the user drags back.

---

## 7. History and presets

Three states, in the order the builder resolves them.

1. **`Swatches::Hidden`** — no row. The default.
2. **`Swatches::Given(&[Color])`** — the app owns the row. Clicking a chip
   sets the colour. The widget never writes to it; the app pushes what it
   wants on `committed`.
3. **`Swatches::Owned`** — the widget keeps a `History` in its own state.

```rust
#[derive(Debug)]
struct History {
    colors: ArrayVec<[Color; History::CAP]>, // tinyvec, already a dependency
}

impl History {
    const CAP: usize = 16;
    /// Seeded with the presets, so the row is never empty and never changes
    /// length.
    fn new() -> Self;
    /// Move to front, de-duplicate, drop the tail. Called on `committed`.
    fn push(&mut self, c: Color);
}
```

The brief says an empty history shows presets. Seeding the history with the
presets is that rule with a stable row length: the first pick pushes to the
front and the sixteenth preset falls off the end.

### 7.1 The default presets

Sixteen colours, derived rather than hand-picked, so the row agrees with the
model the picker uses.

- **Twelve hues** at `Okhsv { h: i / 12, s: 1.0, v: 1.0 }` for `i` in `0..12`.
  Even spacing in Okhsv hue is even spacing to the eye, which is the whole
  argument for the model. Each one sits on the sRGB gamut edge.
- **Four neutrals** at `Okhsv { h: 0.0, s: 0.0, v }` for `v` in
  `[0.0, 0.35, 0.7, 1.0]` — black, two greys, white.

Built once into the `History` when the state is created. `Okhsv::to_color` is
not `const`, so the list is computed, not baked. Sixteen conversions on first
open is not a cost worth a `LazyLock`.

---

## 8. Layout

The panel from the HTML proposal, top to bottom. A `vstack` with
`theme.gap` between rows.

```
┌ field ──────────────────────────────────┐   ColorField
└─────────────────────────────────────────┘
┌ chip ┐ ┌ hue bar ───────────────────────┐   ColorSwatch + ColorStrip::hue
│      │ └────────────────────────────────┘
└──────┘ ┌ alpha bar ─────────────────────┐   ColorStrip::alpha
         └────────────────────────────────┘
┌ hex ─────────┐┌ A % ─┐┌ H ° ─┐              TextEdit + DragValue + DragValue
┌ R ─┐┌ G ─┐┌ B ─┐┌ S % ─┐                    four DragValues
[ Okhsv | HSV ]                               two Buttons, pressed state
[■][■][■][■][■][■][■][■]                      ColorSwatch row
```

- The hex field and the two bars share the panel width. The chip is
  `theme.chip_size` square and the bars fill the rest of the row.
- The numeric grid is four equal columns. Hex spans two.
- Every number is a `DragValue`:
  - `R`, `G`, `B` — `&mut i64`, range `0..=255`, speed 1.
  - `A` — `&mut i64`, range `0..=100`, suffix `"%"`.
  - `H` — `&mut i64`, range `0..=360`, suffix `"°"`.
  - `S` — `&mut i64`, range `0..=100`, suffix `"%"`.
- The values are frame-local `i64`s read out of the colour before the row and
  applied after it. `H` and `S` write straight into `coords`. `R`, `G`, `B`
  rebuild `coords` through `ColorCoords::new`. `A` writes the alpha.
- `DragValue::editable(true)` is already the way to type an exact number, so
  the row needs no second input mode.
- The hex field is a `TextEdit` with `max_chars(7)`. It parses on the change
  edge. A string that does not parse snaps back to the current colour and
  reports nothing.

The model row shows only when both models are worth offering, which is
always. Two `Button`s, the active one drawn pressed. `ComboBox` would be a
heavier control for a two-way switch.

---

## 9. Theme

```rust
/// What a colour picker wears: the two surfaces it paints, the handle that
/// rides them, and the checker behind anything translucent.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ColorPickerTheme {
    pub field_width: f32,
    pub field_height: f32,
    pub bar_thickness: f32,
    pub chip_size: f32,
    pub swatch_size: f32,
    pub handle_radius: f32,
    pub handle_width: f32,
    pub handle_outer: Color,
    pub handle_inner: Color,
    pub checker_light: Color,
    pub checker_dark: Color,
    pub checker_cell: f32,
    pub border: Color,
    pub border_width: f32,
    pub gap: f32,
}

impl ColorPickerTheme {
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            field_width: 208.0,
            field_height: 160.0,
            bar_thickness: 14.0,
            chip_size: 38.0,
            swatch_size: 18.0,
            handle_radius: 6.0,
            handle_width: 1.5,
            handle_outer: Color::linear_rgba(0.0, 0.0, 0.0, 0.75),
            handle_inner: Color::linear_rgba(1.0, 1.0, 1.0, 0.95),
            checker_light: p.elem_mid,
            checker_dark: p.elem,
            checker_cell: 6.0,
            border: p.elem_strong,
            border_width: 1.0,
            gap: 6.0,
        }
    }
}

palette_default!(ColorPickerTheme);
```

Slot `color_picker` on `Theme`. Read every length through
`f32::themed_length(min)`, so a hand-edited theme file cannot produce a
negative field.

The downsample factor is **not** a theme value. It trades accuracy against CPU
time, which is a correctness parameter with a measured bound, so it is a
constant with a builder override and the test that pins it.

---

## 10. Interaction

### Pointer

Copied from `Slider`, including the mapping against last frame's arranged
rect. Press jumps, drag tracks, the release frame sets `committed`.

### Keyboard

Focus gates every key, read inside the widget's own record, the way
`TabStrip::keyboard_travel` does it.

| Chord | Field | Bar |
| --- | --- | --- |
| Arrow | ±0.005 on one axis | ±0.005 |
| Shift + arrow | ±0.05 | ±0.05 |
| Home / End | saturation 0 / 1 | 0 / 1 |
| Page up / down | value 1 / 0 | ±0.1 |

Sample every chord with `ui.key_pressed`, never short-circuit. `key_pressed`
both reads the press and keeps the chord subscribed for the wake gate, so one
firing must not drop another's subscription that frame.

**Each key press commits.** The proposal said a commit on key release, which
`key_pressed` cannot express — it is a press edge and the crate exposes no
release. A held arrow therefore writes one undo entry per auto-repeat step. If
that reads badly in the hand, the fix is an idle timer in the picker state,
and it is deferred until someone complains.

### Popup

`ColorButton` opens `Popup::below(trigger_rect)` with
`ClickOutside::Dismiss`. Esc and an outside press both close. The colour keeps
its last committed value; there is no revert, because every gesture already
committed.

---

## 11. Public surface

`lib.rs`, in the existing alphabetical blocks:

```rust
pub use primitives::color::color_coords::ColorCoords;
pub use primitives::color::color_model::ColorModel;
pub use primitives::color::hsv::Hsv;
pub use primitives::color::okhsv::Okhsv;
pub use widgets::color_button::ColorButton;
pub use widgets::color_field::ColorField;
pub use widgets::color_picker::ColorPicker;
pub use widgets::color_strip::ColorStrip;
pub use widgets::color_swatch::ColorSwatch;
pub use widgets::theme::color_picker::ColorPickerTheme;
```

`ColorSurface` stays `pub(crate)`.

---

## 12. Tests

Inline `mod tests` per file until it passes 150 lines or 40 % of the file,
then `mod tests;` beside it.

### `okhsv.rs`

- **Corners are exact.** The six sRGB corner colours round-trip to the hues
  `0.0812052, 0.3049145, 0.3958204, 0.5410249, 0.7334778, 0.9121206`, and
  `Okhsv { h, s: 1.0, v: 1.0 }.to_color()` returns each corner within 1/255.
  Tolerance 1e-5 on the hue. The constants are measured, so the test is what
  keeps them honest — it derives them and compares.
- **Round trip.** `Color → Okhsv → Color` over a fixed 9×9×9 grid, worst
  channel error under 1e-4. The reference port measures 1e-6.
- **Gamut clamp.** `Okhsv { h: 0.0812, s: 1.0, v: 1.0 }` returns no channel
  below zero. Without the clamp the port returns −1/255.
- **Grey keeps the fallback.** `Okhsv::from_color(grey, 0.25).h == 0.25`, and
  `s == 0.0`.
- **Ends are absolute.** `v = 0` is black for every hue and saturation.
  `s = 0, v = 1` is white for every hue.

### `hsv.rs`

- **Encoded, not linear.** `Hsv { h: 0.0, s: 1.0, v: 0.5 }.to_color()` is
  `#800000`, not `#BB0000`. Hand-computed: HSV value is an sRGB-encoded
  component, so `0.5` encodes to 128, and the linear value behind it is
  0.2140. Assert on `to_srgb_u8`.
- Corners, round trip and grey, as above.

### `color_coords.rs`

- **The model switch keeps the colour.** `with_model` round-trips a colour
  through both models within 1/255.
- **The model switch keeps grey's hue.** A grey coordinate switched and
  switched back reports the hue it started with.

### `color_field/tests.rs`

- **Corners map exactly.** A press at each corner of the field yields
  `(s, v)` of `(0,1)`, `(1,1)`, `(0,0)`, `(1,0)` with no half-pixel drift.
- **Texture accuracy.** Build the field texels at factor 4 for twelve hues,
  bilinear-sample them at every physical pixel, and compare against
  `Okhsv::to_color`. Assert the worst error under **5/255**. Parameterized:
  factor 16 must fail the same bound, which is what proves the parameter
  matters.
- **The texture is sRGB-encoded.** The texel at `s = 0, v = 0.5` is 128, not
  188. This is the one test that catches a `ColorU8::from(Color)` slipping in
  where `to_srgb_u8` belongs.
- **Rebuild is keyed.** Two frames at the same hue register one image and
  update it zero times. A hue change updates it once.
- **Keys move the axes.** Arrow right at `s = 0.5` gives `0.505`. Shift gives
  `0.55`. End gives `1.0`. Held at the end, `changed` is false.
- **Commit is an edge.** `changed` only when the value moved, `committed` only
  on the release frame.

### `color_strip/tests.rs`

- Alpha bar writes only alpha, never the three colour channels.
- The checker is baked: the texel under a transparent stretch alternates
  between the two checker colours.

### `color_picker/tests.rs`

- **Hue survives grey.** Drag value to 0, then back up. The hue is the hue it
  was.
- **An outside edit re-seeds.** Write a new colour into the binding between
  frames. The handles move to it.
- **The numeric row round-trips.** Set `R` to 200 through the drag value, read
  the colour, and get 200 back out of `to_srgb_u8`.
- **Hex parses and rejects.** `"#4CD3FF"` sets the colour, `"zzz"` snaps back
  and reports nothing.
- **History seeds, moves to front and de-duplicates.** Sixteen presets, a
  commit puts the colour first, a repeat commit does not lengthen the row.
- **Presets are the derived list.** The twelfth preset is
  `Okhsv { h: 11.0/12.0, s: 1.0, v: 1.0 }`.

### Suites

- `tests/visual` — one golden per model for the panel, and one for the popup.
  Rendering changes need this suite; the unit tests will not catch a shader or
  a colour-space regression.
- `tests/alloc` — a gate that drags the hue bar for 30 frames and allocates
  nothing after warmup. This is the test that keeps `ColorSurface` honest.

---

## 13. Benches

`widgets/color_picker/bench.rs`, gated `#[cfg(feature = "internals")]`.

- `okhsv_to_color` — one conversion.
- `field_texels_4` — a full 79 × 61 fill, which is the hue-drag frame cost.
  This is the number that decides whether factor 4 stays the default.

Name the target when running it: `cargo bench -p palantir --bench color_picker`.
The root `[profile.bench]` is fat-LTO, so an unfiltered run links every bench
target at once.

---

## 14. Phases

Each phase ends green and is worth shipping alone.

**Phase 0 — image refresh.** §4.3. Registry queue, backend `write_texture`,
`ImageHandle::update`, tests for the queue and the free list. No picker code.

**Phase 1 — colour maths.** `okhsv.rs`, `hsv.rs`, `color_coords.rs`,
`color_model.rs`, with §12's first three test groups. No widget.

**Phase 2 — theme and swatch.** `ColorPickerTheme`, the `color_picker` slot,
`ColorSwatch` with its checker. The smallest painting slice.

**Phase 3 — field and bars.** `ColorSurface`, `ColorField`, `ColorStrip`, the
accuracy tests, and a `colors` page in the showcase.

**Phase 4 — the panel.** `ColorPicker`: layout, hex, drag values, model
switch, history and presets.

**Phase 5 — the trigger.** `ColorButton` and its popup, the visual goldens,
the alloc gate, the bench.

### Verification, every phase

```
cargo fmt -p palantir \
  && cargo clippy -p palantir --all-targets --all-features -- -D warnings \
  && cargo test -p palantir --lib --tests --all-features
```

Phases 0, 3, 4 and 5 move pixels, so they also run the visual suite.

---

## 15. Deferred, and why

- **A colour-field brush kind in the quad shader.** Exact per pixel with no
  CPU work and no upload. The texture path already reaches 3.9/255, so this
  buys accuracy nobody can see. Revisit only if the field cost fails its
  bench.
- **Okhsl as a third model.** Its perceptual lightness axis is the better one
  for building a palette, and its fold only breaks the *square*. Offer it
  later as three bars — H, S, L — where one dimension per bar cannot fold.
- **The eyedropper.** Sampling the screen on Wayland needs a portal and a
  permission dialog. Ship no button rather than a button that fails.
- **A model-channel row.** The numeric grid shows hex, R, G, B, A, H and S.
  A switch between RGB and the model's own three channels is a later
  convenience.
- **Commit on key release.** §10.
