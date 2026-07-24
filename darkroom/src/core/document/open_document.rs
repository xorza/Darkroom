//! The document currently open in a frontend, with its persistence path.

use std::path::{Path, PathBuf};

use crate::core::document::Document;
use crate::core::io::document::{self, DocumentLoadError, DocumentSaveError};

#[derive(Debug, Default)]
pub(crate) struct OpenDocument {
    pub(crate) document: Document,
    pub(crate) path: Option<PathBuf>,
}

impl OpenDocument {
    pub(crate) fn load(path: PathBuf) -> Result<Self, DocumentLoadError> {
        let document = document::load(&path)?;
        Ok(Self {
            document,
            path: Some(path),
        })
    }

    pub(crate) fn save_to(&mut self, path: &Path) -> Result<(), DocumentSaveError> {
        document::save(&self.document, path)?;
        self.path = Some(path.to_path_buf());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::core::document::open_document::OpenDocument;
    use crate::core::io::document::DocumentLoadError;

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
    fn empty_document_has_the_main_graph_tab() {
        let open = OpenDocument::default();

        assert!(open.path.is_none());
        assert_eq!(open.document.layout.all_tabs().count(), 1);
    }
}
