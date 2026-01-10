use crate::model::ViewGraph;
use common::FileFormat;

const UNDO_FILE_FORMAT: FileFormat = FileFormat::Lua;

#[derive(Debug, Default)]
pub struct UndoStack {
    undo_stack: Vec<String>,
    redo_stack: Vec<String>,
}

impl UndoStack {
    pub fn reset_with(&mut self, view_graph: &ViewGraph) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.push_current(view_graph);
    }

    pub fn push_current(&mut self, view_graph: &ViewGraph) {
        let snapshot = view_graph.serialize(UNDO_FILE_FORMAT);
        if self.undo_stack.last().is_some_and(|last| last == &snapshot) {
            return;
        }
        self.undo_stack.push(snapshot);
    }

    pub fn clear_redo(&mut self) {
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> Option<ViewGraph> {
        if self.undo_stack.len() < 2 {
            return None;
        }

        let current = self
            .undo_stack
            .pop()
            .expect("undo stack should contain current snapshot");
        self.redo_stack.push(current);

        let snapshot = self
            .undo_stack
            .last()
            .expect("undo stack should contain a prior snapshot");
        Some(deserialize_snapshot(snapshot))
    }

    pub fn redo(&mut self) -> Option<ViewGraph> {
        let snapshot = self.redo_stack.pop()?;
        self.undo_stack.push(snapshot.clone());
        Some(deserialize_snapshot(&snapshot))
    }
}

fn deserialize_snapshot(snapshot: &str) -> ViewGraph {
    ViewGraph::deserialize(UNDO_FILE_FORMAT, snapshot)
        .expect("undo snapshot should deserialize into a ViewGraph")
}
