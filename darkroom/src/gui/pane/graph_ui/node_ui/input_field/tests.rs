use scenarium::internals::stub_func;
use scenarium::{
    Binding, ConstValue, CustomTypeId, DataType, FuncId, GhostBinding, InputDecl, InputPort,
    InputState, NodeId, OutputDecl, PortId,
};

use crate::core::document::internals::DocumentFixture;
use crate::gui::graph_ctx::internals::GraphCtxFixture;
use crate::gui::pane::graph_ui::node_ui::input_field::InputField;

/// Which case a field is, and the constant one carries.
fn label(field: InputField<'_>) -> String {
    match field {
        InputField::Const(value) => format!("Const {}", value.value_text()),
        InputField::Empty => "Empty".to_owned(),
        InputField::Wired(_) => "Wired".to_owned(),
        InputField::Unwired => "Unwired".to_owned(),
        InputField::KeptConst(value) => format!("KeptConst {}", value.value_text()),
        InputField::KeptWire => "KeptWire".to_owned(),
    }
}

/// The probe's Int input `a`.
fn a(node: NodeId) -> InputPort {
    InputPort::new(node, PortId::new(1))
}

/// A wire from `producer`'s output.
fn wire(producer: NodeId) -> Binding {
    Binding::wire(producer, PortId::new(1))
}

fn three() -> Binding {
    Binding::Const(ConstValue::Int(3))
}

/// Each of the six cases, from the binding and the ghost on the port: a constant, nothing where a
/// constant could sit, a wire, nothing where none could, and a kept constant and a kept wire on an
/// input that holds nothing.
#[test]
fn an_input_reads_as_one_of_six_fields() {
    // A producer and a node, each with an Int input `a` and an input of a type no constant holds.
    let fields = |state: fn(NodeId, NodeId) -> Option<(InputPort, InputState)>| {
        let image = DataType::Custom(CustomTypeId::from_u128(41));
        let func = stub_func(FuncId::unique(), "two")
            .input(1, InputDecl::optional("a", DataType::Int))
            .input(2, InputDecl::optional("image", image))
            .output(1, OutputDecl::new("out", DataType::Int))
            .build();
        let mut fixture = DocumentFixture::default();
        let [producer, node] = [fixture.add(&func), fixture.add(&func)];
        if let Some((port, state)) = state(producer, node) {
            fixture
                .document
                .graphs
                .main_mut()
                .set_input_state(port, state);
        }
        let scene = GraphCtxFixture::over(fixture);
        let ctx = scene.graph_ctx();
        let node = ctx.node(node).expect("the node is placed");
        node.inputs()
            .map(|input| label(InputField::of(input)))
            .collect::<Vec<_>>()
    };

    assert_eq!(fields(|_, _| None), ["Empty", "Unwired"]);
    assert_eq!(
        fields(|_, node| Some((a(node), InputState::Live(three())))),
        ["Const 3", "Unwired"]
    );
    assert_eq!(
        fields(|producer, node| Some((a(node), InputState::Live(wire(producer))))),
        ["Wired", "Unwired"]
    );
    assert_eq!(
        fields(|_, node| Some((a(node), InputState::Ghost(GhostBinding::from(three()))))),
        ["KeptConst 3", "Unwired"]
    );
    assert_eq!(
        fields(|producer, node| {
            Some((
                a(node),
                InputState::Ghost(GhostBinding::from(wire(producer))),
            ))
        }),
        ["KeptWire", "Unwired"]
    );
    // A wire on the input no constant can hold.
    assert_eq!(
        fields(|producer, node| {
            Some((
                InputPort::new(node, PortId::new(2)),
                InputState::Live(wire(producer)),
            ))
        }),
        ["Empty", "Wired"]
    );
}
