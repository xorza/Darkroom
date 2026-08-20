# Issues

- `imaginarium/src/image/conversion/simd/` — the luminance kernels weight RGB
  with the 8-bit fixed-point `LUMA_8BIT` (sum 256, `>> 8`) where the scalar
  reference uses the 16-bit `LUMA_R`/`LUMA_G`/`LUMA_B` (sum 65536, `>> 16`), so
  a SIMD `L_U8` byte can sit one unit off the reference's. Every other kernel in
  the crate is bit-identical to the reference it stands in for.
