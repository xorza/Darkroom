use common::FileFormat;
use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub struct UndoStack<T> {
    undo_bytes: Vec<u8>,
    redo_bytes: Vec<u8>,
    undo_stack: Vec<std::ops::Range<usize>>,
    redo_stack: Vec<std::ops::Range<usize>>,
    format: FileFormat,
    _marker: std::marker::PhantomData<T>,
}

impl<T> UndoStack<T>
where
    T: Serialize + DeserializeOwned,
{
    pub fn new(format: FileFormat) -> Self {
        Self {
            undo_bytes: Vec::new(),
            redo_bytes: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            format,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn reset_with(&mut self, value: &T) {
        self.undo_bytes.clear();
        self.redo_bytes.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.push_current(value);
    }

    pub fn push_current(&mut self, value: &T) {
        let snapshot = serialize_snapshot(value, self.format);
        if self
            .undo_stack
            .last()
            .is_some_and(|last| snapshot_matches(&self.undo_bytes, last, &snapshot))
        {
            return;
        }
        let range = append_bytes(&mut self.undo_bytes, &snapshot);
        self.undo_stack.push(range);
    }

    pub fn clear_redo(&mut self) {
        self.redo_bytes.clear();
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> Option<T> {
        if self.undo_stack.len() < 2 {
            return None;
        }

        let current = self
            .undo_stack
            .pop()
            .expect("undo stack should contain current snapshot");
        let current_bytes = slice_from_range(&self.undo_bytes, &current).to_vec();
        let redo_range = append_bytes(&mut self.redo_bytes, &current_bytes);
        self.redo_stack.push(redo_range);
        pop_tail_bytes(&mut self.undo_bytes, &current);

        let snapshot = self
            .undo_stack
            .last()
            .expect("undo stack should contain a prior snapshot");
        Some(deserialize_snapshot(
            slice_from_range(&self.undo_bytes, snapshot),
            self.format,
        ))
    }

    pub fn redo(&mut self) -> Option<T> {
        let snapshot = self.redo_stack.pop()?;
        let snapshot_bytes = slice_from_range(&self.redo_bytes, &snapshot).to_vec();
        let undo_range = append_bytes(&mut self.undo_bytes, &snapshot_bytes);
        self.undo_stack.push(undo_range);
        pop_tail_bytes(&mut self.redo_bytes, &snapshot);
        Some(deserialize_snapshot(&snapshot_bytes, self.format))
    }
}

fn serialize_snapshot<T: Serialize>(value: &T, format: FileFormat) -> Vec<u8> {
    let serialized = common::serialize(value, format);
    compress_prepend_size(serialized.as_bytes())
}

fn deserialize_snapshot<T: DeserializeOwned>(snapshot: &[u8], format: FileFormat) -> T {
    let decompressed =
        decompress_size_prepended(snapshot).expect("undo snapshot should decompress");
    let decoded = String::from_utf8(decompressed).expect("undo snapshot should be valid UTF-8");
    common::deserialize(&decoded, format).expect("undo snapshot should deserialize into a value")
}

fn append_bytes(target: &mut Vec<u8>, bytes: &[u8]) -> std::ops::Range<usize> {
    let start = target.len();
    target.extend_from_slice(bytes);
    let end = target.len();
    start..end
}

fn slice_from_range<'a>(bytes: &'a [u8], range: &std::ops::Range<usize>) -> &'a [u8] {
    &bytes[range.clone()]
}

fn snapshot_matches(bytes: &[u8], range: &std::ops::Range<usize>, snapshot: &[u8]) -> bool {
    slice_from_range(bytes, range) == snapshot
}

fn pop_tail_bytes(bytes: &mut Vec<u8>, range: &std::ops::Range<usize>) {
    if range.end == bytes.len() {
        bytes.truncate(range.start);
    }
}
