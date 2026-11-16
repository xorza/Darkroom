//! Export: a subgraph and those it places written as a library document, each path it holds
//! bundled into the archive or stripped.
//!
//! - **An export lists every path the definition holds, and each is bundled or stripped**, with
//!   its size; stripped is the default, as an absolute path names the author's system and
//!   nothing on the receiver's. An output path is always stripped, as it names no file to bundle.
//! - **An export is a library document**: the definitions, the exported one marked shared, and a
//!   main graph holding one instance of it, where its author may wire it to sample inputs. As
//!   Blender's asset library file, an ordinary blend-file. With a file bundled it is one
//!   `.darkroomz`, as Blender packs external files into the blend-file
//!   ([packed data](https://docs.blender.org/manual/en/latest/files/blend/packed_data.html));
//!   with none, a document of either form. Refused: a folder of the document and its files, as
//!   Unreal's Migrate writes, which is two things to send.
//! - **A path the receiver must choose is an interface input.** Its value sits on the instance,
//!   outside the definition, and an export has nothing to strip.
//! - **Export is a Graph menu command on the selected instance, else the subgraph the pane
//!   shows**, beside Make Unique and Remove Unused, and asks in a modal dialog, as Save does.

use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::{
    Binding, ConstValue, DataType, FileMode, Graph, GraphId, GraphSet, Library, Node, PortId,
    SubgraphId,
};

use common::fs;

use crate::core::document::Document;
use crate::core::document::graph_view::NodePlacement;

/// One path the definitions an export writes hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportPath {
    pub(crate) path: String,
    /// The file's size when it is there to read; `None` for one that is not.
    pub(crate) size: Option<u64>,
    /// A path an input writes to, which an export always strips.
    pub(crate) output: bool,
}

/// Each path subgraph `id` of `document` and each subgraph it places hold as a constant, each
/// once, sorted, with its size read off the disk: what the export dialog lists.
pub(crate) fn export_paths(
    document: &Document,
    id: SubgraphId,
    library: &Library,
) -> Vec<ExportPath> {
    let mut paths: Vec<ExportPath> = Vec::new();
    for placed in document.graphs.placed_first(id) {
        let graph = document.graphs.graph(GraphId::Subgraph(placed));
        for (&port, binding) in &graph.bindings {
            let Binding::Const(value) = binding else {
                continue;
            };
            let output = writes(graph.find(port.node_id), port.port, library);
            for path in value.as_fs_paths().into_iter().flatten() {
                if path.is_empty() || paths.iter().any(|held| held.path == *path) {
                    continue;
                }
                paths.push(ExportPath {
                    path: path.clone(),
                    size: fs::metadata(path).ok().map(|metadata| metadata.len()),
                    output,
                });
            }
        }
    }
    paths.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    paths
}

/// Whether `port` of `node` is an input that names a file to write.
fn writes(node: Option<&Node>, port: PortId, library: &Library) -> bool {
    node.and_then(Node::func_id)
        .and_then(|func| library.by_id(func))
        .and_then(|func| func.ports().input(port))
        .is_some_and(|input| {
            matches!(
                input.data_type,
                DataType::File {
                    mode: FileMode::Save,
                    ..
                }
            )
        })
}

/// The library document an export of `document`'s subgraph `id` writes: it and each subgraph it
/// places, with their views, `id` marked shared, each path stripped but the inputs' paths in
/// `bundled`, and a main graph holding one instance of `id`.
pub(crate) fn exported(
    document: &Document,
    id: SubgraphId,
    library: &Library,
    bundled: &BTreeSet<String>,
) -> Document {
    let mut graphs = GraphSet::default();
    let placed = document.graphs.placed_first(id);
    for &held in &placed {
        let mut subgraph = document
            .graphs
            .subgraph(held)
            .expect("a placed subgraph is held")
            .clone();
        strip(&mut subgraph.graph, library, bundled);
        graphs.insert_subgraph(held, subgraph);
    }
    let instance = Node::instance(id);
    let node_id = graphs.main_mut().add(instance);
    let mut exported = Document::from(graphs);
    exported.main_view.node_placements.insert(
        node_id,
        NodePlacement {
            pos: Vec2::ZERO,
            z: 0,
        },
    );
    for held in placed {
        let side = exported
            .sides
            .get_mut(&held)
            .expect("each subgraph has a side");
        side.view = document.view(GraphId::Subgraph(held)).clone();
        side.shared = held == id;
    }
    exported
}

/// Take out of `graph` each path constant but an input's in `bundled`: a list keeps the paths it
/// bundles, and a binding left with none goes.
fn strip(graph: &mut Graph, library: &Library, bundled: &BTreeSet<String>) {
    let ports: Vec<_> = graph.bindings.keys().copied().collect();
    for port in ports {
        let output = writes(graph.find(port.node_id), port.port, library);
        let kept = |path: &String| !output && bundled.contains(path);
        let Some(Binding::Const(value)) = graph.bindings.get_mut(&port) else {
            continue;
        };
        let gone = match value {
            ConstValue::FsPath(path) => !path.is_empty() && !kept(path),
            ConstValue::FsPaths(paths) => {
                paths.retain(kept);
                paths.is_empty()
            }
            _ => false,
        };
        if gone {
            graph.bindings.remove(&port);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use common::{TempDir, fs};
    use scenarium::{
        Binding, ConstValue, DataType, FileKindId, FileMode, FuncId, GraphId, InputDecl, InputPort,
        Interface, Library, Node, Subgraph, SubgraphId, internals, numbered_port,
    };

    use crate::core::document::internals::{DocumentFixture, insert_subgraph};
    use crate::core::io::export::{ExportPath, export_paths, exported};

    /// The dialog lists each path of the definition once with its size, an output marked; the
    /// export keeps a bundled input's path, strips an unbundled one and every output, marks the
    /// definition shared and places one instance of it.
    #[test]
    fn an_export_lists_each_path_and_strips_what_it_does_not_bundle() {
        let dir = TempDir::new("darkroom-export");
        let (kept, dropped) = (dir.join("kept.fits"), dir.join("dropped.fits"));
        fs::write(&kept, b"12345").unwrap();
        fs::write(&dropped, b"12").unwrap();
        let file = |mode| DataType::File {
            kind: FileKindId::from_u128(1),
            mode,
        };
        let func = internals::stub_func(FuncId::unique(), "io")
            .input(1, InputDecl::optional("read", file(FileMode::Open)))
            .input(2, InputDecl::optional("also", file(FileMode::Open)))
            .input(3, InputDecl::optional("write", file(FileMode::Save)))
            .build();
        let mut library = Library::default();
        library.add(func.clone());
        let mut subgraph = Subgraph::new("Calibrate", Interface::default());
        let node = subgraph.graph.add(Node::from(&func));
        let path = |p: &Path| p.to_str().unwrap().to_string();
        let port = |n| InputPort::new(node, numbered_port(n));
        for (n, value) in [
            (1, path(&kept)),
            (2, path(&dropped)),
            (3, "/out/x.fits".to_string()),
        ] {
            subgraph
                .graph
                .bindings
                .insert(port(n), Binding::Const(ConstValue::FsPath(value)));
        }
        let id = SubgraphId::from_u128(1);
        let mut fixture = DocumentFixture::default();
        insert_subgraph(&mut fixture.document, id, subgraph);
        let document = fixture.document;

        assert_eq!(
            export_paths(&document, id, &library),
            [
                ExportPath {
                    path: "/out/x.fits".to_string(),
                    size: None,
                    output: true
                },
                ExportPath {
                    path: path(&dropped),
                    size: Some(2),
                    output: false
                },
                ExportPath {
                    path: path(&kept),
                    size: Some(5),
                    output: false
                },
            ]
        );
        let bundled = BTreeSet::from([path(&kept), "/out/x.fits".to_string()]);
        let written = exported(&document, id, &library, &bundled);
        let bindings = &written.graphs.graph(GraphId::Subgraph(id)).bindings;
        assert_eq!(
            bindings.get(&port(1)),
            Some(&Binding::Const(ConstValue::FsPath(path(&kept))))
        );
        assert_eq!(bindings.get(&port(2)), None, "unbundled, stripped");
        assert_eq!(bindings.get(&port(3)), None, "an output is always stripped");
        assert!(written.sides[&id].shared);
        assert_eq!(
            written
                .graphs
                .main()
                .iter()
                .filter(|n| n.subgraph() == Some(id))
                .count(),
            1
        );
        written.validate().unwrap();
    }
}
