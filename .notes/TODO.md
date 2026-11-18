- move the macOS `application:openURLs:` hook (`darkroom/src/platform/macos/open_files.rs`) into palantir's winit host as `App::open_files` once palantir moves past winit 0.30 and its `objc2-app-kit` 0.2

- stacking frames (outer, inner, reference)
- auto chosen reference frame for stacking and manually specified
- visual node groups

- palantir: a split sized in pixels, so the sidebar keeps its width as the window grows; the path of a split, so the sidebar's ratio holds where its split is not the dock's root
- dropping a file on the canvas, once palantir reports a dropped file
- align, distribute and snap; a minimap; a histogram and channel views in the viewer
- a light theme, through ayu-graphite
