//! The criterion benches, which `benches/darkroom.rs` runs: `cargo drive bench`.
//!
//! The kinds, the id shape and the sampling are palantir's (`palantir::bench::kind` and `palantir::bench::backends`), as
//! darkroom's one case is palantir's frame through the editor's widgets: an `integration` case
//! times a whole editor frame, and its id is `<kind>/<group>/<backend>/<scenario>`.

use criterion::Criterion;
use palantir::bench::backends::Backends;

use crate::gui::app::session;

/// Every case. The host's adapters are listed once here; no device opens.
pub fn run(c: &mut Criterion) {
    let backends = Backends::probe();
    session::bench::integration(c, backends);
}
