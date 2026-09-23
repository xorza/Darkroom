# darkroom

The node-graph editor, built on Palantir — our in-tree immediate-mode GUI
library. Both are pre-1.0 and break freely; a change to the editor may require
a coordinated change in the GUI library.

`cargo run` from the workspace root launches it — `darkroom` is the default
member. `cargo run -p darkroom --features profile-with-tracy` forwards
Palantir's Tracy backend; the editor declares no zones of its own yet.
