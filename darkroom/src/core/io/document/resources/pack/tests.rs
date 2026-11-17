use common::{TempDir, fs};
use scenarium::{
    Binding, Compound, CompoundId, ConstValue, DataType, FileKindId, FileMode, FuncId, GraphId,
    InputDecl, InputPort, Interface, Node, PortId, internals,
};

use crate::core::document::internals::{DocumentFixture, insert_compound};
use crate::core::io::document::resources::pack::{PackedInput, pack};

/// A node reading one file on input 1 and several on 2, writing one on 3, and reading a folder on
/// 4: a pack copies `a.png` and `b.png` once each into `<digest>/<name>`, read-only, and names the
/// copies on the two reading inputs alone, `a.png`'s twice-named copy once, and none in a locked
/// compound; a second pack finds every file in the folder and changes nothing.
#[test]
fn a_pack_copies_each_file_an_input_reads_once_and_names_the_copies() {
    let dir = TempDir::new("darkroom-pack");
    let file = |name: &str, bytes: &[u8]| {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        path.to_str().unwrap().to_owned()
    };
    let (a, b) = (file("a.png", b"aaaa"), file("b.png", b"bb"));
    let out = dir.path().join("out.png").to_str().unwrap().to_owned();
    let kind = |mode| DataType::File {
        kind: FileKindId::from_u128(1),
        mode,
    };
    let reader = internals::stub_func(FuncId::unique(), "reader")
        .input(1, InputDecl::optional("one", kind(FileMode::Open)))
        .input(2, InputDecl::optional("many", kind(FileMode::OpenMany)))
        .input(3, InputDecl::optional("write", kind(FileMode::Save)))
        .input(4, InputDecl::optional("folder", DataType::Dir))
        .build();
    let mut fixture = DocumentFixture::default();
    let node = fixture.add(&reader);
    let port = |id| InputPort::new(node, PortId::new(id));
    let graph = fixture.document.graphs.main_mut();
    graph.bindings.clear();
    graph.set_input_binding(port(1), Binding::Const(ConstValue::FsPath(a.clone())));
    graph.set_input_binding(
        port(2),
        Binding::Const(ConstValue::FsPaths(vec![a, b.clone()])),
    );
    graph.set_input_binding(port(3), Binding::Const(ConstValue::FsPath(out)));
    graph.set_input_binding(
        port(4),
        Binding::Const(ConstValue::FsPath(dir.path().to_str().unwrap().to_owned())),
    );
    // A locked compound reading `b.png` too, which takes no edit, so the pack leaves it.
    let locked = CompoundId::from_u128(9);
    let mut inner = Compound::new("locked", Interface::default());
    let inner_node = inner.graph.add(Node::from(&reader));
    inner.graph.set_input_binding(
        InputPort::new(inner_node, PortId::new(1)),
        Binding::Const(ConstValue::FsPath(b)),
    );
    insert_compound(&mut fixture.document, locked, inner);
    fixture.document.sides.get_mut(&locked).unwrap().locked = true;
    let root = dir.path().join("doc.darkroom-resources");

    let packed = pack(&fixture.document, &fixture.library, &root).unwrap();
    let copy = |bytes: &[u8], name: &str| {
        root.join(blake3::hash(bytes).to_hex().as_str())
            .join(name)
            .to_str()
            .unwrap()
            .to_owned()
    };
    let (a_copy, b_copy) = (copy(b"aaaa", "a.png"), copy(b"bb", "b.png"));
    assert_eq!(
        packed,
        [
            PackedInput {
                graph: GraphId::Main,
                port: port(1),
                value: ConstValue::FsPath(a_copy.clone()),
            },
            PackedInput {
                graph: GraphId::Main,
                port: port(2),
                value: ConstValue::FsPaths(vec![a_copy.clone(), b_copy]),
            },
        ]
    );
    assert_eq!(fs::read(&a_copy).unwrap(), b"aaaa");
    assert!(fs::metadata(&a_copy).unwrap().permissions().readonly());
    let digests = fs::read_dir(&root).unwrap().count();
    assert_eq!(digests, 2, "one folder for each content, no staging left");

    let mut moved = fixture.document.clone();
    for PackedInput { graph, port, value } in packed {
        moved
            .graphs
            .graph_mut(graph)
            .set_input_binding(port, Binding::Const(value));
    }
    assert_eq!(
        pack(&moved, &fixture.library, &root).unwrap(),
        [],
        "a packed document packs nothing more"
    );
}
