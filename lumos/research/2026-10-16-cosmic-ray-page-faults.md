# Cosmic-ray pass: allocation and page faults (2026-10-16)

Release build, Ryzen 7 6800U, x86-64-v3, one 6000 × 4000 light.

- `atomic/calibration/cosmic_ray/frame/reject/{mono,rggb}/typical`: 1.55 s per light.
- Seven 96 MB `f32` planes, allocated and first touched: 64 ms. The same seven planes kept and
  refilled: 48 ms. The page faults cost the difference, 17 ms, about 1.1 % of the pass.
- The detector's working set: 580 MB at 6000 × 4000, 720 MB at 6144².
- `calibrate`'s own per-light allocations: the dark-scale fit's hot-pixel pairs, one entry per hot
  pixel, and a 1-bit mask copy when the light holds `NO_DATA` (3 MB at 24 MP).

Practice elsewhere:

- astroscrappy's `detect_cosmics`, which ccdproc calls, allocates about five full-frame arrays per
  call and reuses none across frames
  ([source](https://github.com/astropy/astroscrappy/blob/main/astroscrappy/astroscrappy.pyx)).
- Caching allocators keep freed buffers where an allocation is costly: PyTorch's CUDA cache, with
  `empty_cache()` to give the memory back
  ([docs](https://pytorch.org/docs/stable/generated/torch.cuda.empty_cache)), and OpenCV's
  `cuda::BufferPool`, which cuts `cudaMalloc` calls
  ([docs](https://docs.opencv.org/4.x/d5/d08/classcv_1_1cuda_1_1BufferPool.html)).
- On the CPU, glibc unmaps a large freed block, so the next allocation faults it in again; Eigen
  measured up to 2.2× on small GEMMs, where the faults are a large share of the work
  ([issue](https://gitlab.com/libeigen/eigen/-/issues/3201),
  [mallopt(3)](https://man.archlinux.org/man/mallopt.3.en)).
