use crate::model::ViewGraph;
use common::FileFormat;
use lz4_flex::{compress_prepend_size, decompress_size_prepended};

const UNDO_FILE_FORMAT: FileFormat = FileFormat::Lua;

#[derive(Debug, Default)]
pub struct UndoStack {
    undo_stack: Vec<Vec<u8>>,
    redo_stack: Vec<Vec<u8>>,
}

impl UndoStack {
    pub fn reset_with(&mut self, view_graph: &ViewGraph) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.push_current(view_graph);
    }

    pub fn push_current(&mut self, view_graph: &ViewGraph) {
        let snapshot = serialize_snapshot(view_graph);
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

fn serialize_snapshot(view_graph: &ViewGraph) -> Vec<u8> {
    let serialized = view_graph.serialize(UNDO_FILE_FORMAT);
    compress_prepend_size(serialized.as_bytes())
}

fn deserialize_snapshot(snapshot: &[u8]) -> ViewGraph {
    let decompressed =
        decompress_size_prepended(snapshot).expect("undo snapshot should decompress");
    let decoded = String::from_utf8(decompressed).expect("undo snapshot should be valid UTF-8");
    ViewGraph::deserialize(UNDO_FILE_FORMAT, &decoded)
        .expect("undo snapshot should deserialize into a ViewGraph")
}
