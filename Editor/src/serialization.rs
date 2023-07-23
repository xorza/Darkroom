use eframe::egui;
use serde::{Deserialize, Serialize};

use graph_lib::function::Function;
use graph_lib::graph::{Graph, NodeId};

use crate::app::GraphState;
use crate::eng_integration::{build_node_from_func, combobox_inputs_from_function, EditorNode, EditorState, register_node};

type Positions = Vec<(NodeId, (f32, f32))>;


pub(crate) fn save(
    graph_state: &GraphState,
    editor_state: &EditorState,
    filename: &str,
) -> anyhow::Result<()>
{
    let filename = common::get_file_extension(filename)
        .ok()
        .and_then(|ext| {
            if ext == "yaml" {
                Some(filename.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| format!("{}.yaml", filename));

    let positions: Positions = editor_state.graph.nodes
        .iter()
        .map(|(editor_node_id, editor_node)| {
            let position = editor_state.node_positions.get(editor_node_id).unwrap();
            (editor_node.user_data.node_id, (position.x, position.y))
        })
        .collect();

    let file = std::fs::File::create(filename)?;
    let mut writer = serde_yaml::Serializer::new(file);
    graph_state.graph.serialize(&mut writer)?;
    positions.serialize(&mut writer)?;

    Ok(())
}

pub(crate) fn load(
    functions: &Vec<Function>,
    filename: &str,
) -> anyhow::Result<(GraphState, EditorState)>
{
    let file = std::fs::File::open(filename)?;
    let mut deserializer = serde_yaml::Deserializer::from_reader(file);

    let mut editor_state = EditorState::default();

    let graph = Graph::deserialize(deserializer.next().unwrap())?;
    let positions = Positions::deserialize(deserializer.next().unwrap())?;

    let mut graph_state = GraphState {
        graph,
        arg_mapping: Default::default(),
    };

    drop(deserializer);

    for node in graph_state.graph.nodes() {
        let function = functions
            .iter()
            .find(|function| function.self_id == node.function_id)
            .unwrap();

        let mut combobox_inputs = combobox_inputs_from_function(function);

        combobox_inputs
            .iter_mut()
            .for_each(|combo_input| {
                let value = node
                    .inputs[combo_input.input_index as usize]
                    .const_value
                    .as_ref();
                if let Some(value) = value {
                    combo_input.current_value = value.clone();
                }
            });

        let editor_node = EditorNode {
            node_id: node.id(),
            function_id: node.function_id,
            is_output: node.is_output,
            cache_outputs: node.cache_outputs,
            combobox_inputs,
            trigger_id: Default::default(),
            inputs: vec![],
            events: vec![],
            outputs: vec![],
        };

        let eng_node_id = editor_state.graph.add_node(
            node.name.clone(),
            editor_node,
            |_, _| {},
        );
        editor_state.node_order.push(eng_node_id);

        build_node_from_func(
            &mut editor_state.graph,
            function,
            eng_node_id,
        );

        let arg_mapping = &mut graph_state.arg_mapping;
        let editor_node = &editor_state.graph.nodes[eng_node_id].user_data;
        register_node(editor_node, arg_mapping);

        // Set default values
        node.inputs
            .iter()
            .zip(editor_node.inputs.iter())
            .for_each(|(input, eng_input_id)| {
                if let Some(const_value) = &input.const_value {
                    editor_state.graph.inputs[*eng_input_id].value.0 = const_value.clone();
                }
            });

        positions
            .iter()
            .find(|(id, _)| *id == node.id())
            .map(|(_, (x, y))| {
                editor_state.node_positions.insert(eng_node_id, egui::Pos2 { x: *x, y: *y });
            });
    }

    // Connect inputs to outputs
    graph_state.graph.nodes()
        .iter()
        .flat_map(|node|
            node.inputs
                .iter()
                .enumerate()
                .map(|(index, input)|
                    (node.id(), index, &input.binding)
                )
        )
        .filter(|(_node_id, _index, binding)|
            binding.is_output_binding()
        )
        .for_each(|(node_id, index, binding)| {
            let index = index as u32;

            let input_id = graph_state.arg_mapping.find_input_id(&node_id, index);

            let output_id = binding
                .as_output_binding()
                .map(|output_binding| {
                    graph_state.arg_mapping.find_output_id(
                        &output_binding.output_node_id,
                        output_binding.output_index,
                    )
                })
                .unwrap();

            editor_state.graph.add_connection(output_id, input_id);
        });

    // Connect events to triggers
    graph_state.graph.nodes()
        .iter()
        .flat_map(|node| {
            let event_node_id = node.id();
            node.events
                .iter()
                .enumerate()
                .flat_map(move |(event_index, event)|
                    event.subscribers
                        .iter()
                        .map(move |subscriber_node_id|
                            (event_node_id, event_index, subscriber_node_id)
                        )
                )
        })
        .for_each(|(event_node_id, event_index, subscriber_node_id)| {
            let event_id = graph_state.arg_mapping
                .find_event_id(&event_node_id, event_index as u32);
            let trigger_id = graph_state.arg_mapping
                .find_trigger_id(subscriber_node_id);

            editor_state.graph.add_connection(event_id, trigger_id);
        });

    Ok((graph_state, editor_state))
}
