- stacking frames (outer, inner, reference)
- auto chosen reference frame for stacking and manually specified
- visual node groups

- ghosts: replace a ghost node with another func, with a rule for which ports correspond
- ghosts: a func that names the func it replaces, with a port map the library declares
- ghosts: the names of types the library lacks in the manifest, so a ghost port reads its type
- ghosts: a ghost subscription to a ghost node drawn, which needs a pin on a ghost node

- subgraphs: event ports on an interface
- subgraphs: a cache mode on an instance
- subgraphs: one computed value shared by instances whose digests are equal
- subgraphs: several `Inputs` nodes in one subgraph
- subgraphs: a linked library, and a locked or password-protected subgraph
- subgraphs: pack resources into a document from a command of its own, apart from an export

- move the macOS `application:openURLs:` hook (`darkroom/src/platform/macos/open_files.rs`) into palantir's winit host as `App::open_files` once palantir moves past winit 0.30 and its `objc2-app-kit` 0.2
