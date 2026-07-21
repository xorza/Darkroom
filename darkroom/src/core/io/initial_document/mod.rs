//! Shared startup document selection for every frontend.

use crate::core::io::preferences::Preferences;
use crate::core::io::project;
use crate::core::open_document::OpenDocument;

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub(crate) struct InitialDocument {
    pub(crate) open_document: OpenDocument,
    pub(crate) load_error: Option<anyhow::Error>,
}

pub(crate) fn load(preferences: &Preferences) -> InitialDocument {
    let Some(path) = preferences
        .load_last_document
        .then_some(preferences.document_path.as_deref())
        .flatten()
    else {
        return InitialDocument {
            open_document: OpenDocument::empty(),
            load_error: None,
        };
    };

    match project::load(path) {
        Ok(document) => InitialDocument {
            open_document: OpenDocument::new(document, Some(path.to_path_buf())),
            load_error: None,
        },
        Err(error) => InitialDocument {
            open_document: OpenDocument::empty(),
            load_error: Some(error),
        },
    }
}
