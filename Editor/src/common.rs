use egui_node_graph as eng;
use graph_lib::data::StaticValue;
use graph_lib::function::Function;

use crate::eng_integration::{EditorGraph, EditorValue};

pub(crate) fn build_node_from_func(
    editor_graph: &mut EditorGraph,
    function: &Function,
    eng_node_id: eng::NodeId,
) {
    function.inputs
        .iter()
        .filter(|input| input.variants.is_none())
        .for_each(|input| {
            let shown_inline = input.variants.is_none();
            let param_kind = if input.data_type.is_custom() {
                eng::InputParamKind::ConnectionOnly
            } else {
                eng::InputParamKind::ConnectionOrConstant
            };

            let _input_id = editor_graph.add_input_param(
                eng_node_id,
                input.name.to_string(),
                input.data_type.clone(),
                EditorValue(StaticValue::from(&input.data_type)),
                param_kind,
                shown_inline,
            );
        });

    function.outputs
        .iter()
        .for_each(|output| {
            let _output_id = editor_graph.add_output_param(
                eng_node_id,
                output.name.to_string(),
                output.data_type.clone(),
            );
        });
}

