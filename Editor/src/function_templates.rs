use std::borrow::Cow;

use egui_node_graph as eng;
use graph_lib::function::{Function, FunctionId};
use graph_lib::graph::NodeId;

use crate::app::AppState;
use crate::eng_integration::{build_node_from_func, combobox_inputs_from_function, EditorDataType, EditorGraph, EditorNode, EditorValue};

pub(crate) struct NodeCategory(String);

#[derive(Clone, Debug, Default)]
pub(crate) struct FunctionTemplate(Function);

#[derive(Default, Clone, Debug)]
pub(crate) struct FunctionTemplates {
    templates: Vec<FunctionTemplate>,
}


impl FunctionTemplates {
    fn function_by_id(&self, id: FunctionId) -> Option<&Function> {
        self.templates.iter().find(|f| f.0.self_id == id).map(|f| &f.0)
    }
}

impl From<Vec<Function>> for FunctionTemplates {
    fn from(functions: Vec<Function>) -> Self {
        let templates = functions.iter()
            .map(|f| FunctionTemplate(f.clone()))
            .collect();

        Self { templates }
    }
}

impl eng::NodeTemplateIter for FunctionTemplates {
    type Item = FunctionTemplate;

    fn all_kinds(&self) -> &Vec<Self::Item> {
        &self.templates
    }
}

impl eng::NodeTemplateTrait for FunctionTemplate {
    type NodeData = EditorNode;
    type DataType = EditorDataType;
    type ValueType = EditorValue;
    type UserState = AppState;
    type CategoryType = NodeCategory;

    fn node_finder_label(&self, _user_state: &mut Self::UserState) -> Cow<'_, str> {
        self.0.name.as_str().into()
    }

    fn node_finder_categories(&self, _user_state: &mut Self::UserState) -> Vec<Self::CategoryType> {
        vec![NodeCategory(self.0.category.clone())]
    }

    fn node_graph_label(&self, user_state: &mut Self::UserState) -> String {
        self.node_finder_label(user_state).into()
    }

    fn user_data(&self, _user_state: &mut Self::UserState) -> Self::NodeData {
        let combobox_inputs = combobox_inputs_from_function(&self.0);

        EditorNode {
            function_id: self.0.self_id,
            node_id: NodeId::nil(),
            is_output: self.0.is_output,
            cache_outputs: false,
            combobox_inputs,
            trigger_id: Default::default(),
            inputs: vec![],
            events: vec![],
            outputs: vec![],
        }
    }

    fn build_node(
        &self,
        graph: &mut EditorGraph,
        _user_state: &mut Self::UserState,
        node_id: eng::NodeId,
    ) {
        let function = &self.0;
        build_node_from_func(
            graph,
            function,
            node_id,
        );
    }
}

impl eng::CategoryTrait for NodeCategory {
    fn name(&self) -> String {
        self.0.clone()
    }
}

