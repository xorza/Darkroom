use eframe::egui;
use serde::{Deserialize, Serialize};

use graph_lib::graph::{Graph, NodeId};
use graph_lib::invoke::Invoker;

use crate::app::{ArgAddress, GraphState};
use crate::common::build_node_from_func;
use crate::eng_integration::{EditorNode, EditorState};

type Positions = Vec<(NodeId, (f32, f32))>;


pub(crate) fn save(
    editor_state: &EditorState,
    graph: &Graph,
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
    graph.serialize(&mut writer)?;
    positions.serialize(&mut writer)?;

    Ok(())
}

pub(crate) fn load(
    invoker: &dyn Invoker,
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
        input_mapping: vec![],
        output_mapping: vec![],
    };

    drop(deserializer);

    for node in graph_state.graph.nodes() {
        let function = invoker.function_by_id(node.function_id);
        let editor_node = EditorNode {
            node_id: node.id(),
            function_id: node.function_id,
            is_output: node.is_output,
            cache_outputs: node.cache_outputs,
        };

        let eng_node_id = editor_state.graph.add_node(
            node.name.clone(),
            editor_node,
            |_, _| {},
        );
        editor_state.node_order.push(eng_node_id);

        build_node_from_func(
            &mut editor_state.graph,
            &function,
            eng_node_id,
        );

        let eng_node = &editor_state.graph.nodes[eng_node_id];
        eng_node.inputs
            .iter()
            .enumerate()
            .for_each(|(index, (_name, input_id))| {
                graph_state.input_mapping.push((
                    *input_id,
                    ArgAddress {
                        node_id: node.id(),
                        arg_index: index as u32,
                    },
                ));
            });
        eng_node.outputs
            .iter()
            .enumerate()
            .for_each(|(index, (_name, output_id))| {
                graph_state.output_mapping.push((
                    *output_id,
                    ArgAddress {
                        node_id: node.id(),
                        arg_index: index as u32,
                    },
                ));
            });

        // set default values
        node.inputs
            .iter()
            .zip(eng_node.inputs.iter())
            .for_each(|(input, (_name, eng_input_id))| {
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
            let input_address = ArgAddress {
                node_id,
                arg_index: index as u32,
            };
            let output_address = binding
                .as_output_binding()
                .map(|output_binding| {
                    ArgAddress {
                        node_id: output_binding.output_node_id,
                        arg_index: output_binding.output_index,
                    }
                }).unwrap();

            let input_id = graph_state.input_mapping
                .iter()
                .find(|(_, address)| *address == input_address)
                .map(|(input_id, _)| *input_id)
                .unwrap();
            let output_id = graph_state.output_mapping
                .iter()
                .find(|(_, address)| *address == output_address)
                .map(|(output_id, _)| *output_id)
                .unwrap();

            editor_state.graph.add_connection(output_id, input_id);
        });

    Ok((graph_state, editor_state))
}
