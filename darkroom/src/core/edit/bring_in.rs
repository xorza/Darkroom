//! [`bring_in`]: definitions from another file entering a document, as a paste and a place from
//! the library take them in.
//!
//! - **Nothing but an explicit Update changes a definition the document holds.** One equal to the
//!   definition held under its id, by digest, is that definition; a different one comes in beside
//!   it under a fresh id, and what came with it names that one. As Cargo builds two copies of a
//!   dependency two packages need at incompatible versions. Refused: one version per document,
//!   which changes a compound nobody touched.
//! - **A definition that enters from another file is parsed against this library**, as a load
//!   parses a document ([`GraphSet::reconcile`]), with the manifest its file held: a node its
//!   graph cannot hold here stays a ghost node that says what it was, so the definition travels
//!   whole. Refused: taking it in as the other file held it, which leaves the document a graph
//!   the compile refuses.
//! - **A definition that enters from another file loses its output paths**, as a write inside a
//!   compound is one the receiver did not pick and cannot see; the input takes the receiver's
//!   library default, as a new node's input does. A node pasted onto the canvas keeps its own,
//!   which is in view.

use std::collections::HashMap;

use scenarium::{
    Binding, Compound, CompoundDigest, CompoundId, DataType, FileMode, GraphSet, InputPort,
    InputState, Library, Manifest, NodeId,
};

use crate::core::document::Document;
use crate::core::document::compound_side::CompoundSide;
use crate::core::document::graph_view::GraphView;
use crate::core::edit::compound_intent::NewCompound;

/// A definition from another file, under the id that file held it under, with its view and the
/// digest it last shared with the library there.
#[derive(Debug)]
pub(crate) struct Carried {
    pub(crate) id: CompoundId,
    pub(crate) compound: Compound,
    pub(crate) view: GraphView,
    pub(crate) synced: Option<CompoundDigest>,
    /// Its author's lock, which travels with every copy.
    pub(crate) locked: bool,
}

/// What a [`bring_in`] adds, in an order whose insertions each hold, and the id each carried
/// definition goes by in the document.
#[derive(Debug)]
pub(crate) struct BroughtIn {
    pub(crate) compounds: Vec<NewCompound>,
    ids: HashMap<CompoundId, CompoundId>,
}

impl BroughtIn {
    /// The id the definition carried under `id` goes by in the document; `id` itself for one it
    /// did not carry.
    pub(crate) fn id(&self, id: CompoundId) -> CompoundId {
        self.ids.get(&id).copied().unwrap_or(id)
    }
}

/// Make each definition of `carried` one `library` can run, as a load makes a document's, read
/// against `manifest`, the one of the file they came from: together, as each resolves the
/// instances it places against the others. The set they make is handed back for its digests.
pub(crate) fn reconcile(
    carried: &mut [Carried],
    library: &Library,
    manifest: &Manifest,
) -> GraphSet {
    let mut set = GraphSet::default();
    for definition in &*carried {
        set.insert_compound(definition.id, definition.compound.clone());
    }
    for (graph, repairs) in set.reconcile(library, manifest).repairs {
        for repair in repairs {
            tracing::info!(target: "darkroom::edit", "bringing in {graph:?}: {repair}");
        }
    }
    for definition in carried {
        definition.compound.clone_from(
            set.compound(definition.id)
                .expect("each carried definition is in the set"),
        );
    }
    set
}

/// Take `carried`, each definition after those it places, into `document` by the rules in the
/// module doc, reconciled against `library` and `manifest`, their file's, and its output paths
/// stripped.
pub(crate) fn bring_in(
    document: &Document,
    library: &Library,
    mut carried: Vec<Carried>,
    manifest: &Manifest,
) -> BroughtIn {
    let set = reconcile(&mut carried, library, manifest);
    let mut ids: HashMap<CompoundId, CompoundId> = HashMap::new();
    let mut incoming = Vec::new();
    for definition in carried {
        let id = match document.graphs.compound(definition.id) {
            Some(_) if document.graphs.digest(definition.id) == set.digest(definition.id) => {
                ids.insert(definition.id, definition.id);
                continue;
            }
            Some(_) => CompoundId::unique(),
            None => definition.id,
        };
        ids.insert(definition.id, id);
        incoming.push((id, definition));
    }
    let compounds = incoming
        .into_iter()
        .map(|(id, definition)| {
            let mut compound = definition.compound;
            enter(&mut compound, &ids, library);
            let side = CompoundSide {
                view: definition.view,
                synced: definition.synced,
                locked: definition.locked,
                ..CompoundSide::new(&compound.graph)
            };
            NewCompound {
                id,
                compound,
                side,
                type_names: manifest.types().clone(),
            }
        })
        .collect();
    BroughtIn { compounds, ids }
}

/// Ready `compound`, carried in from another file, for the document: each instance pointed at
/// the id `ids` gives the compound it places, and its output paths stripped against `library`.
fn enter(compound: &mut Compound, ids: &HashMap<CompoundId, CompoundId>, library: &Library) {
    // Read before the graph is written.
    let nodes: Vec<NodeId> = compound.graph.iter().map(|node| node.node_id).collect();
    for node_id in nodes {
        let node = compound.graph.find_mut(node_id).expect("listed above");
        if let Some(placed) = node.compound() {
            node.set_compound(ids.get(&placed).copied().unwrap_or(placed));
        }
    }
    strip_output_paths(compound, library);
    take_path_defaults(compound, library);
}

/// Put the library's default on each path input of `compound`'s func nodes that holds nothing, as
/// a new node's input takes it: an input an export stripped follows the receiver's preferences,
/// a model path among them.
fn take_path_defaults(compound: &mut Compound, library: &Library) {
    let graph = &mut compound.graph;
    let mut defaults = Vec::new();
    for node in graph.iter() {
        let Some(func) = node.func_id().and_then(|id| library.by_id(id)) else {
            continue;
        };
        for input in func.inputs() {
            let port = InputPort::new(node.node_id, input.id);
            let path = matches!(input.data_type, DataType::Dir | DataType::File { .. });
            if path
                && graph.input_state(port) == InputState::Unbound
                && let Some(default) = &input.default_value
            {
                defaults.push((port, default.clone()));
            }
        }
    }
    for (port, default) in defaults {
        graph.set_input_binding(port, Binding::Const(default));
    }
}

impl BroughtIn {
    /// Ready `compound`, carried in beside what this brought in, for the document, as each
    /// definition it brought in was: what an update does with the definition it puts in place.
    pub(crate) fn enter(&self, compound: &mut Compound, library: &Library) {
        enter(compound, &self.ids, library);
    }
}

/// Take each constant out of an input of `compound`'s func nodes that names a file to write,
/// putting the library's default in its place where the func declares one.
fn strip_output_paths(compound: &mut Compound, library: &Library) {
    let graph = &mut compound.graph;
    let outputs: Vec<_> = graph
        .wiring()
        .bindings()
        .filter(|(_, binding)| matches!(binding, Binding::Const(_)))
        .filter_map(|(port, _)| {
            let func = library.by_id(graph.find(port.node_id)?.func_id()?)?;
            let input = func.ports().input(port.port)?;
            matches!(
                input.data_type,
                DataType::File {
                    mode: FileMode::Save,
                    ..
                }
            )
            .then(|| (port, input.default_value.clone()))
        })
        .collect();
    for (port, default) in outputs {
        graph.set_input_binding(port, default.map(Binding::Const));
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{
        Binding, Compound, CompoundId, ConstValue, DataType, FileKindId, FileMode, FuncId,
        InputDecl, InputPort, Interface, Library, Manifest, Node, PortId, internals,
    };

    use crate::core::document::Document;
    use crate::core::document::graph_view::GraphView;
    use crate::core::edit::bring_in::{Carried, bring_in};

    /// A definition entering from another file loses the constant on an input that names a file
    /// to write, keeps the one on an input that names a file to read, and an empty path input
    /// takes the library's default.
    #[test]
    fn a_definition_that_enters_loses_its_output_paths() {
        let file = |mode| DataType::File {
            kind: FileKindId::from_u128(1),
            mode,
        };
        let writer = internals::stub_func(FuncId::unique(), "writer")
            .input(1, InputDecl::optional("read", file(FileMode::Open)))
            .input(2, InputDecl::optional("write", file(FileMode::Save)))
            .input(
                3,
                InputDecl::optional("model", file(FileMode::Open))
                    .default(ConstValue::FsPath("/models/here.onnx".to_string())),
            )
            .build();
        let mut library = Library::default();
        library.add(writer.clone());
        let mut compound = Compound::new("export", Interface::default());
        let node = compound.graph.add(Node::from(&writer));
        let path = |name: &str| Binding::Const(ConstValue::FsPath(format!("/data/{name}")));
        let (read, write) = (
            InputPort::new(node, PortId::new(1)),
            InputPort::new(node, PortId::new(2)),
        );
        compound.graph.set_input_binding(read, path("in.tif"));
        compound.graph.set_input_binding(write, path("out.tif"));
        let view = GraphView::new(&compound.graph);
        let id = CompoundId::from_u128(1);
        let brought = bring_in(
            &Document::default(),
            &library,
            vec![Carried {
                id,
                compound,
                view,
                synced: None,
                locked: false,
            }],
            &Manifest::default(),
        );
        let [entered] = brought.compounds.as_slice() else {
            panic!("one definition comes in");
        };
        assert_eq!(entered.id, id, "an id the document lacks is kept");
        let bindings = entered.compound.graph.wiring();
        assert_eq!(bindings.binding(read), Some(&path("in.tif")));
        assert_eq!(bindings.binding(write), None);
        assert_eq!(
            bindings.binding(InputPort::new(node, PortId::new(3))),
            Some(&Binding::Const(ConstValue::FsPath(
                "/models/here.onnx".to_string()
            ))),
            "a path input left empty takes the receiver's default"
        );
    }
}
