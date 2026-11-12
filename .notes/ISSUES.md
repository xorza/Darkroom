# Open issues

- `imaginarium`'s u8 blend kernel (`BlendRow<u8>::run`) depends on codegen units for its speed. Under 16 units (`release-dev`, or `release` with `codegen-units = 16`), the `[_; 4].map` closures and `core::array::Drain` stay out of line in the AVX2 loop. The `atomic/blend/cpu/avx2_fma/*/*_u8` cases then run about 3× slower and retire about 4.8× the instructions they do at one unit.
- `imaginarium`'s scalar conversions from a mono format into `mono_u16`, `rgb_u16` and `rgba_u16` (`atomic/convert/cpu/scalar/*` and the matching `integration/convert/cpu/*` cases) retire 1.5–2.3× the instructions under `release`'s fat LTO that they do under thin LTO, at one codegen unit or sixteen. `integration/convert/cpu/mono_u8/rgb_u16` takes 1.45× the time.

- `darkroom/src/gui/state/preview_store` never checks `ImageHandle::is_resident`. After the window host recovers from a lost GPU device, every preview thumbnail and full texture paints nothing, and the store keeps no texels or source to upload again once a full texture is resident. A full texture also keeps the size `Ui::max_image_dimension` capped it to on the lost device.
- `statrs` calls std's libm in `Normal` and `StudentsT`, which `lumos`'s normal scores and sigma bounds read, so those can still differ by a last bit per system.
