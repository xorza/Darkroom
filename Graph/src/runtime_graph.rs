use std::any::Any;

use serde::{Deserialize, Serialize};

use crate::data::Value;
use crate::graph::{FunctionBehavior, NodeId};

#[derive(Debug, Default)]
pub struct InvokeContext {
    boxed: Option<Box<dyn Any>>,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct RuntimeNode {
    pub(crate) node_id: NodeId,

    pub name: String,
    pub is_output: bool,
    pub has_missing_inputs: bool,
    pub behavior: FunctionBehavior,
    pub should_execute: bool,
    pub should_cache_outputs: bool,
    pub run_time: f64,

    #[serde(skip)]
    pub(crate) invoke_context: InvokeContext,
    #[serde(skip)]
    pub(crate) output_values: Option<Vec<Option<Value>>>,
    pub(crate) output_binding_count: Vec<u32>,
    pub(crate) total_binding_count: u32,
}


#[derive(Default, Serialize, Deserialize)]
pub struct RuntimeGraph {
    pub nodes: Vec<RuntimeNode>,
}


impl RuntimeNode {
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }

    pub(crate) fn increment_binding_count(&mut self, output_index: u32) {
        self.output_binding_count[output_index as usize] += 1;
        self.total_binding_count += 1;
    }
    pub(crate) fn decrement_binding_count(&mut self, output_index: u32) {
        assert!(self.output_binding_count[output_index as usize] >= 1);
        assert!(self.total_binding_count >= 1);

        self.output_binding_count[output_index as usize] -= 1;
        self.total_binding_count -= 1;
    }
}

impl RuntimeGraph {
    pub fn node_by_name(&self, name: &str) -> Option<&RuntimeNode> {
        self.nodes.iter().find(|&p_node| p_node.name == name)
    }

    pub fn node_by_id(&self, node_id: NodeId) -> Option<&RuntimeNode> {
        self.nodes.iter()
            .find(|&p_node| p_node.node_id == node_id)
    }
    pub fn node_by_id_mut(&mut self, node_id: NodeId) -> Option<&mut RuntimeNode> {
        self.nodes.iter_mut()
            .find(|p_node| p_node.node_id == node_id)
    }
}


impl InvokeContext {
    pub(crate) fn default() -> InvokeContext {
        InvokeContext {
            boxed: None,
        }
    }

    pub fn is_none(&self) -> bool {
        self.boxed.is_none()
    }

    pub fn is_some<T>(&self) -> bool
    where T: Any + Default
    {
        match &self.boxed {
            None => false,
            Some(v) => v.is::<T>(),
        }
    }

    pub fn get<T>(&self) -> Option<&T>
    where T: Any + Default
    {
        self.boxed.as_ref()
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    pub fn get_mut<T>(&mut self) -> Option<&mut T>
    where T: Any + Default
    {
        self.boxed.as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    pub fn set<T>(&mut self, value: T)
    where T: Any + Default
    {
        self.boxed = Some(Box::new(value));
    }

    pub fn get_or_default<T>(&mut self) -> &mut T
    where T: Any + Default
    {
        let is_some = self.is_some::<T>();

        if is_some {
            self.boxed
                .as_mut()
                .unwrap()
                .downcast_mut::<T>()
                .unwrap()
        } else {
            self.boxed
                .insert(Box::<T>::default())
                .downcast_mut::<T>()
                .unwrap()
        }
    }
}


