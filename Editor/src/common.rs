use egui_node_graph as eng;
use graph_lib::data::StaticValue;
use graph_lib::function::Function;

use crate::eng_integration::{ComboboxInput, EditorGraph, EditorValue};

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

            let value = input.default_value
                .as_ref()
                .map(|value| {
                    value.clone()
                })
                .unwrap_or_else(|| {
                    StaticValue::from(&input.data_type)
                });

            let _input_id = editor_graph.add_input_param(
                eng_node_id,
                input.name.to_string(),
                input.data_type.clone(),
                EditorValue(value),
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

pub(crate) fn combobox_inputs_from_function(function: &Function) -> Vec<ComboboxInput> {
    let combobox_inputs = function.inputs
        .iter()
        .enumerate()
        .filter_map(|(index, input)| {
            if let Some(variants) = input.variants.as_ref() {
                let variants = variants
                    .iter()
                    .map(|variant| (variant.0.clone(), variant.1.clone()))
                    .collect::<Vec<(StaticValue, String)>>();
                let current_value = input.default_value.as_ref()
                    .unwrap_or_else(|| {
                        &variants
                            .first()
                            .expect("No variants")
                            .0
                    })
                    .clone();

                Some(ComboboxInput {
                    input_index: index as u32,
                    name: input.name.clone(),
                    current_value,
                    variants,
                })
            } else {
                None
            }
        })
        .collect::<Vec<ComboboxInput>>();

    combobox_inputs
}

