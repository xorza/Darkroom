//! [`pack`]: Pack Resources, each file a document reads copied into its resources folder.
//!
//! - **Pack Resources copies each file the document reads into its resources folder** and points
//!   the paths there, as one edit. The folder is the one an archive unpacks into, so a plain
//!   document reads the copies beside it and a save as an archive bundles them, by the rule that
//!   bundles every unpacked file a document names. As Blender's Pack Resources
//!   ([packed data](https://docs.blender.org/manual/en/4.2/files/blend/packed_data.html)), less
//!   its "packing will happen on the next save": here the copy is the pack.
//! - **A file is read once**: streamed into a temporary file in the folder while it is hashed,
//!   then renamed to the name its digest gives, or dropped where a copy of it is there.
//! - **Only a file an input reads is packed**: not a folder, not a path an input writes, and not
//!   what a ghost holds, as no declaration says it names a file to read, as an export strips it.
//!   A file past [`MAX_RESOURCE_BYTES`], which no unpack would take, stays where it is, and so does
//!   one a followed or locked compound reads, which takes no edit.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::slice;

use common::fs;
use scenarium::{
    Binding, ConstValue, DataType, FileMode, Graph, GraphId, InputPort, Library, Scope,
};
use thiserror::Error;

use crate::core::document::Document;
use crate::core::io::document::resources::{
    MAX_RESOURCE_BYTES, RESOURCES, is_file_name, unpacked_entry,
};

/// One input whose constant Pack Resources points at the copies, and what it holds then.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PackedInput {
    pub(crate) graph: GraphId,
    pub(crate) port: InputPort,
    pub(crate) value: ConstValue,
}

/// Why a pack stopped: a file it could not read, or a copy it could not write.
#[derive(Debug, Error)]
pub(crate) enum PackError {
    #[error("could not read {path}", path = .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not write into {path}", path = .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Copy each file `document`'s inputs read into `root`, its resources folder, under
/// `<digest>/<file name>`, and the inputs whose constants then name the copies. A file already in
/// a resources folder, or one that is not there to read, is left as it is.
pub(crate) fn pack(
    document: &Document,
    library: &Library,
    root: &Path,
) -> Result<Vec<PackedInput>, PackError> {
    let reading = reading_inputs(document, library);
    let mut copies: BTreeMap<String, String> = BTreeMap::new();
    for (_, _, value) in &reading {
        for path in value.as_fs_paths().into_iter().flatten() {
            if copies.contains_key(path) || !packs(Path::new(path)) {
                continue;
            }
            let copy = copy_in(Path::new(path), root)?;
            copies.insert(path.clone(), copy);
        }
    }
    let mut packed = Vec::new();
    for (graph, port, value) in reading {
        let mut moved = value.clone();
        let paths: &mut [String] = match &mut moved {
            ConstValue::FsPath(path) => slice::from_mut(path),
            ConstValue::FsPaths(paths) => paths,
            _ => continue,
        };
        let mut changed = false;
        for path in paths {
            if let Some(copy) = copies.get(path.as_str()) {
                path.clone_from(copy);
                changed = true;
            }
        }
        if changed {
            packed.push(PackedInput {
                graph,
                port,
                value: moved,
            });
        }
    }
    Ok(packed)
}

/// Each live constant of `document` on an input that reads files, in graph order, outside a
/// compound that takes no edit: its paths would be copied and every edit of them refused.
fn reading_inputs<'a>(
    document: &'a Document,
    library: &Library,
) -> Vec<(GraphId, InputPort, &'a ConstValue)> {
    let mut reading = Vec::new();
    for (graph, held) in document.graphs.graphs() {
        if document.read_only(graph).is_some() {
            continue;
        }
        let scope = document.graphs.scope(library, graph);
        for (port, binding) in held.wiring().bindings() {
            let Binding::Const(value) = binding else {
                continue;
            };
            if reads_files(scope, held, port) {
                reading.push((graph, port, value));
            }
        }
    }
    reading
}

/// Whether `port` is an input that reads one file or several.
fn reads_files(scope: Scope<'_>, graph: &Graph, port: InputPort) -> bool {
    graph
        .find(port.node_id)
        .and_then(|node| scope.resolve(node).ok())
        .and_then(|decl| decl.ports().input(port.port).map(|input| input.data_type))
        .is_some_and(|data_type| {
            matches!(
                data_type,
                DataType::File {
                    mode: FileMode::Open | FileMode::OpenMany,
                    ..
                }
            )
        })
}

/// Whether the file at `path` is one a pack copies: a regular file not in a resources folder
/// already, within the size an unpack takes.
fn packs(path: &Path) -> bool {
    unpacked_entry(path).is_none()
        && fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_RESOURCE_BYTES)
}

/// Copy the file at `source` into `root` under `<digest>/<file name>`, read once: streamed into a
/// temporary file while hashed, then renamed to the name its digest gives, or dropped where that
/// copy is there. The copy is read-only, as an unpacked file is. Its path.
fn copy_in(source: &Path, root: &Path) -> Result<String, PackError> {
    let read = |error| PackError::Read {
        path: source.to_path_buf(),
        source: error,
    };
    let write = |path: &Path| {
        let path = path.to_path_buf();
        move |error| PackError::Write {
            path,
            source: error,
        }
    };
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| is_file_name(name))
        .unwrap_or(RESOURCES);
    fs::create_dir_all(root).map_err(write(root))?;
    let staging = root.join(format!(".{file_name}.packing"));
    let mut input = fs::open(source).map_err(read)?;
    let mut output = fs::create(&staging).map_err(write(&staging))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let len = input.read(&mut buffer).map_err(read)?;
        if len == 0 {
            break;
        }
        hasher.update(&buffer[..len]);
        output.write_all(&buffer[..len]).map_err(write(&staging))?;
    }
    output.sync_all().map_err(write(&staging))?;
    drop(output);
    let folder = root.join(hasher.finalize().to_hex().as_str());
    let target = folder.join(file_name);
    if fs::is_file(&target) {
        fs::remove_file(&staging).map_err(write(&staging))?;
    } else {
        fs::create_dir_all(&folder).map_err(write(&folder))?;
        fs::rename(&staging, &target).map_err(write(&target))?;
        let mut permissions = fs::metadata(&target).map_err(write(&target))?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&target, permissions).map_err(write(&target))?;
    }
    target
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| PackError::Write {
            path: target.clone(),
            source: io::Error::new(io::ErrorKind::InvalidData, "the path is not UTF-8"),
        })
}

#[cfg(test)]
mod tests;
