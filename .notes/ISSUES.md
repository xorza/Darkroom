# Issues

- imaginarium contrast/brightness: the f32 SIMD row kernels earn nothing over
  the scalar reference on a 25 MP frame — RGB_f32 8.81 ms vs 8.82 ms, RGBA_f32
  11.81 ms vs 11.85 ms, L_f32 2.16 ms vs 2.20 ms. The u8 and u16 kernels win
  4–6× against theirs. All f32 cases sit at ~32 GiB/s.
