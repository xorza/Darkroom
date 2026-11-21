//! [`FuncRedirect`]: a func the library no longer registers, and the one that replaces it.
//!
//! # Decisions
//!
//! - **A func names the func it replaces with a redirect the library holds**, as Unreal's Core
//!   Redirects map an old class, property or function name to its new one so saved assets keep
//!   their data ([Core Redirects](https://dev.epicgames.com/documentation/unreal-engine/core-redirects-in-unreal-engine)):
//!   a load moves each node of the old id, a node or a ghost node, to the new func through the
//!   redirect's port map before it resolves anything, and reports it. Refused: a redirect in the
//!   document, which the author of the func, not of the document, knows; a version number on a
//!   func, which says that it changed, not how.
//! - **Redirects chain, and a chain is bounded**:
//!   [`LibraryBuilder::build`](crate::LibraryBuilder::build) refuses a redirect from a
//!   registered func, one into a loop, and a chain past [`MAX_REDIRECTS`] or ending at no
//!   registered func, and flattens each chain, so a load takes one step for any redirect.

use crate::decl::port_map::NodeRemap;
use crate::identity::FuncId;

/// The most redirects a load follows from one func id to the func it lands on.
pub const MAX_REDIRECTS: usize = 8;

/// A func id the library no longer registers, the func that replaces it, and where each port of
/// the old one went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncRedirect {
    pub from: FuncId,
    pub to: FuncId,
    pub remap: NodeRemap,
}
