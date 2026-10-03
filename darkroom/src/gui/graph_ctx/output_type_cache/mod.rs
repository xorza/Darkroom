//! [`OutputTypeCache`]: the output type table, resolved again only when
//! what it was resolved from changed.

use std::sync::Arc;

use scenarium::{Library, OutputTypes};

use crate::core::document::graph_revision::GraphRevision;
use crate::core::document::open_document::OpenDocument;

/// The open document's resolved output types, kept across frames.
///
/// A table depends on the graph's node set and wiring and on the library's
/// declarations, so it is resolved again only when the document's
/// [`GraphRevision`] or the library moved — an edit's frame pays one resolve,
/// and every other frame pays none.
#[derive(Debug, Default)]
pub(crate) struct OutputTypeCache {
    types: OutputTypes,
    resolved_for: Option<ResolvedFor>,
}

/// What a table was resolved from. The library is held, not compared by
/// address: holding it keeps the address from passing to another library.
#[derive(Debug)]
struct ResolvedFor {
    revision: GraphRevision,
    library: Arc<Library>,
}

impl OutputTypeCache {
    /// The table for `open`'s graph against `library`, resolved again first
    /// when either changed since the last call.
    pub(crate) fn refresh(&mut self, open: &OpenDocument, library: &Arc<Library>) -> &OutputTypes {
        let revision = open.graph_revision();
        let current = self.resolved_for.as_ref().is_some_and(|resolved| {
            resolved.revision == revision && Arc::ptr_eq(&resolved.library, library)
        });
        if !current {
            self.types.update(&open.document.graph, library);
            self.resolved_for = Some(ResolvedFor {
                revision,
                library: Arc::clone(library),
            });
        }
        &self.types
    }
}

#[cfg(test)]
mod tests;
