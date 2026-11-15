- stacking frames (outer, inner, reference)
- auto chosen reference frame for stacking and manually specified
- visual node groups
- subgraphs

- upload images again after a GPU device recovery (`ImageHandle::is_resident`), see `ISSUES.md`. Decided: `DrawablePreview` holds its source `DynamicValue` for life and `land` checks residency each frame; a lost thumbnail converts again from the source, a lost full texture goes back to `Unasked` so a viewer asks again at the new device's texture limit, as Blender rebuilds an image's GPU texture from its buffer (`BKE_image_get_gpu_texture`). Refused: CPU copies of the rasters, which palantir's device-loss decision refuses and whose `update` cannot resize; running the preview nodes again, which costs a run and touches run state
- move the macOS `application:openURLs:` hook (`darkroom/src/platform/macos/open_files.rs`) into palantir's winit host as `App::open_files` once palantir moves past winit 0.30 and its `objc2-app-kit` 0.2
