use std::collections::BTreeSet;
use std::sync::Arc;

use scenarium::testing::graph::{NodeSpec, TestGraph};
use scenarium::{Binding, ConstValue, DataType, InputPort, Library, OutputPort};

use crate::core::document::harness::DocFixture;
use crate::core::document::open_document::OpenDocument;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::relayout::Relayout;
use crate::gui::graph_ctx::output_type_cache::OutputTypeCache;

/// The table follows the graph's revision and the library, and nothing
/// else. A binding written behind the edit pipeline's back is what shows
/// a resolve did *not* run: only a resolve would see it.
#[test]
fn the_table_is_resolved_again_only_when_its_sources_move() {
    // `src`'s `Int` output feeds `hop`, a passthrough: `hop`'s output
    // mirrors whatever its input carries.
    let mut g = TestGraph::new();
    g.add("src", |n| n.output(DataType::Int));
    g.add("hop", NodeSpec::passthrough);
    g.wire("src", 0, "hop", 0);
    let (src, hop) = (g.id("src"), g.id("hop"));
    let DocFixture { doc, library } = g.into();
    let library = Arc::new(library);
    let mut open = OpenDocument::over(doc);
    let mut cache = OutputTypeCache::default();
    let hop_in = InputPort::new(hop, 0);
    let hop_out = OutputPort::new(hop, 0);
    let ty = |cache: &mut OutputTypeCache, open: &OpenDocument, library: &Arc<Library>| {
        cache.refresh(open, library).get(hop_out).cloned()
    };
    assert_eq!(ty(&mut cache, &open, &library), Some(DataType::Int));

    // A selection retypes nothing, so the table is kept: the unbinding
    // written straight into the graph goes unseen.
    assert_eq!(
        open.apply_edit(
            GraphIntent::SetSelection {
                to: BTreeSet::from([src]),
            },
            &library
        ),
        Relayout::NotNeeded
    );
    open.document.graph.set_input_binding(hop_in, None);
    assert_eq!(ty(&mut cache, &open, &library), Some(DataType::Int));

    // A binding edit moves the revision: a `Float` constant on the hop
    // retypes its output.
    let _relayout = open.apply_edit(
        GraphIntent::SetInput {
            input: hop_in,
            to: Some(Binding::Const(ConstValue::Float(1.0))),
        },
        &library,
    );
    assert_eq!(ty(&mut cache, &open, &library), Some(DataType::Float));

    // Undo moves it too, back to the unbound input the step found.
    let _relayout = open.undo();
    assert_eq!(ty(&mut cache, &open, &library), Some(DataType::Any));

    // A library swap resolves again at the same revision: against a
    // library that declares none of the graph's funcs, no port resolves.
    let swapped = Arc::new(Library::default());
    assert_eq!(ty(&mut cache, &open, &swapped), None);
    assert_eq!(ty(&mut cache, &open, &library), Some(DataType::Any));
}
