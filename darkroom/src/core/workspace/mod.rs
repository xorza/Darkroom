//! Shared document/runtime coordination used by GUI and terminal frontends.

use std::path::Path;

use scenarium::NodeId;

use crate::core::document::open_document::OpenDocument;
use crate::core::io::preferences::Preferences;
use crate::core::runtime_host::RuntimeHost;
use crate::core::script::ScriptConfig;
use crate::core::status::StatusLog;
use crate::core::wake::Wake;

#[cfg(test)]
mod tests;

fn load_preferred_document(preferences: &mut Preferences, status: &mut StatusLog) -> OpenDocument {
    load_preferred_document_with(preferences, status, Preferences::save)
}

fn load_preferred_document_with(
    preferences: &mut Preferences,
    status: &mut StatusLog,
    save_preferences: impl FnOnce(&Preferences) -> Result<(), String>,
) -> OpenDocument {
    let Some(path) = preferences
        .document_path
        .clone()
        .filter(|_| preferences.load_last_document)
    else {
        return OpenDocument::default();
    };
    match OpenDocument::load(path) {
        Ok(open) => open,
        Err(error) => {
            status.error(format!("load failed: {error:#}"));
            preferences.document_path = None;
            if let Err(error) = save_preferences(preferences) {
                status.error(error);
            }
            OpenDocument::default()
        }
    }
}

#[derive(Debug)]
pub(crate) struct Workspace {
    pub(crate) open: OpenDocument,
    pub(crate) runtime: RuntimeHost,
}

impl Workspace {
    pub(crate) fn new(
        script_config: &ScriptConfig,
        wake: Wake,
        preferences: &mut Preferences,
    ) -> Self {
        let mut status = StatusLog::default();
        let open = load_preferred_document(preferences, &mut status);
        let mut runtime = RuntimeHost::new(script_config, wake, preferences, status);
        runtime.set_document_cache(open.path.as_deref());
        Self { open, runtime }
    }

    pub(crate) fn replace_document(&mut self, open: OpenDocument) {
        self.open = open;
        self.runtime.set_document_cache(self.open.path.as_deref());
    }

    pub(crate) fn run_once(&mut self) -> bool {
        self.normalize_document();
        self.runtime.run_once(&self.open.document.graph)
    }

    pub(crate) fn run_node(&mut self, node_id: NodeId) -> bool {
        self.normalize_document();
        self.runtime.run_node(&self.open.document.graph, node_id)
    }

    pub(crate) fn start_event_loop(&mut self) -> bool {
        self.normalize_document();
        self.runtime.start_event_loop(&self.open.document.graph)
    }

    pub(crate) fn save_caches(&mut self) {
        self.normalize_document();
        self.runtime.save_caches(&self.open.document.graph);
    }

    pub(crate) fn save_to(&mut self, path: &Path) -> anyhow::Result<()> {
        let library = self.runtime.library.published.load();
        self.open.save_to(path, &library)?;
        self.runtime.set_document_cache(self.open.path.as_deref());
        Ok(())
    }

    pub(crate) fn normalize_document(&mut self) {
        self.open.normalize(&self.runtime.library.published.load());
    }
}
