# Open issues
- `lumos` test `pipeline::tests::calibrated_lights_on_disk_stack_as_held_ones_do` (`pipeline/tests/mod.rs`) takes 1.1 s in a debug build, past the 1 s a test may take.
- `lumos` memory plan of the registered pipeline (`pipeline/light_source.rs`, `LightFiles::stack`): `calibrate_align_stack` of 12 dithered RGGB lights of 2048² with `memory_override` 256 MiB, the reference found, peaks at 1.49 GB resident (`VmHWM`), and a 512 MiB memory cgroup kills it; the plan counts a fraction of what the run holds (`pipeline::bench::parked_lights_under_a_memory_cap` reproduces it).
