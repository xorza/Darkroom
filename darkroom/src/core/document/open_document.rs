//! The document currently open in a frontend, including its persistence path
//! and whether derived graph state needs normalization.

use std::path::{Path, PathBuf};

use scenarium::Library;

use crate::core::document::Document;
use crate::core::io::document::{self, DocumentLoadError};

#[derive(Debug)]
pub(crate) struct OpenDocument {
    pub(crate) document: Document,
    pub(crate) path: Option<PathBuf>,
    pub(crate) normalization_pending: bool,
}

impl OpenDocument {
    pub(crate) fn load(path: PathBuf) -> Result<Self, DocumentLoadError> {
        let document = document::load(&path)?;
        Ok(Self {
            document,
            path: Some(path),
            normalization_pending: true,
        })
    }

    pub(crate) fn normalize(&mut self, library: &Library) {
        if !self.normalization_pending {
            return;
        }
        self.document.graph.normalize(library);
        self.normalization_pending = false;
    }

    pub(crate) fn save_to(&mut self, path: &Path, library: &Library) -> anyhow::Result<()> {
        self.normalize(library);
        document::save(&self.document, path)?;
        self.path = Some(path.to_path_buf());
        Ok(())
    }
}

impl Default for OpenDocument {
    fn default() -> Self {
        Self {
            document: Document::default(),
            path: None,
            normalization_pending: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use scenarium::Graph;
    use scenarium::{Binding, Func, FuncId, FuncInput, InputPort, Library, StaticValue};

    use crate::core::document::Document;
    use crate::core::document::open_document::OpenDocument;
    use crate::core::io::document::DocumentLoadError;

    fn from_document(document: Document) -> OpenDocument {
        OpenDocument {
            document,
            path: None,
            normalization_pending: true,
        }
    }

    #[test]
    fn load_returns_the_document_error() {
        let path = PathBuf::from("not-a-document.json");

        let error = OpenDocument::load(path.clone()).unwrap_err();

        assert!(matches!(
            error,
            DocumentLoadError::InvalidExtension { path: error_path } if error_path == path
        ));
    }

    #[test]
    fn normalization_prunes_stale_wiring_once_for_each_open_document() {
        let func_id = FuncId::unique();
        let previous = Func::new(func_id, "changed")
            .input(FuncInput::required("removed", scenarium::DataType::Float));
        let library = Library::from([Func::new(func_id, "changed")]);
        let document = || {
            let mut graph = Graph::default();
            let node_id = graph.add_func_node(&previous);
            let input = InputPort::new(node_id, 0);
            graph.set_input_binding(input, Binding::Const(StaticValue::Float(3.0)));
            Document::from(graph)
        };
        let mut open = from_document(document());

        assert!(open.normalization_pending);
        assert_eq!(open.document.graph.bindings.len(), 1);
        open.normalize(&library);
        assert!(!open.normalization_pending);
        assert!(open.document.graph.bindings.is_empty());

        open = from_document(document());
        assert!(open.normalization_pending);
        assert_eq!(open.document.graph.bindings.len(), 1);
        open.normalize(&library);
        assert!(!open.normalization_pending);
        assert!(open.document.graph.bindings.is_empty());
    }

    #[test]
    fn empty_document_has_the_main_graph_tab() {
        let open = OpenDocument::default();

        assert!(open.path.is_none());
        assert_eq!(open.document.layout.all_tabs().count(), 1);
        assert!(open.normalization_pending);
    }
}
