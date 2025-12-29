use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFormat {
    Toml,
    Yaml,
    Json,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Graph {
    pub id: Uuid,
    pub nodes: Vec<Node>,
    pub pan: egui::Vec2,
    pub zoom: f32,
    pub selected_node_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    pub name: String,
    pub pos: egui::Pos2,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub cache_output: bool,
    pub has_cached_output: bool,
    // node has side effects, besides calculation it's output. e.g. saving re
    pub terminal: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Connection {
    pub node_id: Uuid,
    pub output_index: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
    pub connection: Option<Connection>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Output {
    pub name: String,
}

impl Default for Node {
    fn default() -> Self {
        let id = Uuid::new_v4();
        let name = format!("Node {}", id);

        Self {
            id,
            name,
            pos: egui::Pos2::ZERO,
            inputs: Vec::new(),
            outputs: Vec::new(),
            cache_output: false,
            has_cached_output: false,
            terminal: false,
        }
    }
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            nodes: Vec::new(),
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            selected_node_id: None,
        }
    }
}

impl Graph {
    pub fn validate(&self) -> Result<()> {
        if !self.zoom.is_finite() || self.zoom <= 0.0 {
            return Err(anyhow!("graph zoom must be finite and positive"));
        }
        if !self.pan.x.is_finite() || !self.pan.y.is_finite() {
            return Err(anyhow!("graph pan must be finite"));
        }

        let mut output_counts = HashMap::new();
        for node in &self.nodes {
            if !node.pos.x.is_finite() || !node.pos.y.is_finite() {
                return Err(anyhow!("node position must be finite"));
            }
            let prior = output_counts.insert(node.id, node.outputs.len());
            if prior.is_some() {
                return Err(anyhow!("duplicate node id detected"));
            }
        }

        if let Some(selected_node_id) = self.selected_node_id
            && !output_counts.contains_key(&selected_node_id)
        {
            return Err(anyhow!("selected node id must exist in graph"));
        }

        for node in &self.nodes {
            for input in &node.inputs {
                if let Some(connection) = &input.connection {
                    let output_count = output_counts
                        .get(&connection.node_id)
                        .ok_or_else(|| anyhow!("connection references a missing node"))?;
                    if connection.output_index >= *output_count {
                        return Err(anyhow!("connection output index out of range"));
                    }
                }
            }
        }

        Ok(())
    }

    pub fn serialize(&self, format: GraphFormat) -> Result<String> {
        self.validate()?;

        match format {
            GraphFormat::Json => serde_json::to_string_pretty(self).map_err(anyhow::Error::from),
            GraphFormat::Yaml => serde_yml::to_string(self).map_err(anyhow::Error::from),
            GraphFormat::Toml => toml::to_string(self).map_err(anyhow::Error::from),
        }
    }

    pub fn deserialize(format: GraphFormat, input: &str) -> Result<Self> {
        if input.trim().is_empty() {
            bail!("graph input is empty");
        }

        let graph = match format {
            GraphFormat::Json => {
                serde_json::from_str::<Graph>(input).map_err(anyhow::Error::from)?
            }
            GraphFormat::Yaml => {
                serde_yml::from_str::<Graph>(input).map_err(anyhow::Error::from)?
            }
            GraphFormat::Toml => toml::from_str::<Graph>(input).map_err(anyhow::Error::from)?,
        };
        graph.validate()?;

        Ok(graph)
    }

    pub fn serialize_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();
        let format = GraphFormat::from_path(path)?;
        let payload = self.serialize(format)?;
        std::fs::write(path, payload).map_err(anyhow::Error::from)
    }

    pub fn deserialize_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let format = GraphFormat::from_path(path)?;
        let payload = std::fs::read_to_string(path).map_err(anyhow::Error::from)?;

        Self::deserialize(format, &payload)
    }

    pub fn test_graph() -> Self {
        let value_a_id = Uuid::new_v4();
        let value_b_id = Uuid::new_v4();
        let sum_id = Uuid::new_v4();
        let divide_id = Uuid::new_v4();
        let output_id = Uuid::new_v4();

        let value_a = Node {
            id: value_a_id,
            name: "value_a".to_string(),
            pos: egui::pos2(80.0, 120.0),
            inputs: Vec::new(),
            outputs: vec![Output {
                name: "value".to_string(),
            }],
            cache_output: true,
            has_cached_output: true,
            terminal: false,
        };

        let value_b = Node {
            id: value_b_id,
            name: "value_b".to_string(),
            pos: egui::pos2(80.0, 260.0),
            inputs: Vec::new(),
            outputs: vec![Output {
                name: "value".to_string(),
            }],
            cache_output: true,
            has_cached_output: true,
            terminal: false,
        };

        let sum = Node {
            id: sum_id,
            name: "math(sum)".to_string(),
            pos: egui::pos2(320.0, 180.0),
            inputs: vec![
                Input {
                    name: "a".to_string(),
                    connection: Some(Connection {
                        node_id: value_a_id,
                        output_index: 0,
                    }),
                },
                Input {
                    name: "b".to_string(),
                    connection: Some(Connection {
                        node_id: value_b_id,
                        output_index: 0,
                    }),
                },
            ],
            outputs: vec![Output {
                name: "sum".to_string(),
            }],
            cache_output: false,
            has_cached_output: false,
            terminal: false,
        };

        let divide = Node {
            id: divide_id,
            name: "math(divide)".to_string(),
            pos: egui::pos2(560.0, 180.0),
            inputs: vec![
                Input {
                    name: "sum".to_string(),
                    connection: Some(Connection {
                        node_id: sum_id,
                        output_index: 0,
                    }),
                },
                Input {
                    name: "b".to_string(),
                    connection: Some(Connection {
                        node_id: value_b_id,
                        output_index: 0,
                    }),
                },
            ],
            outputs: vec![Output {
                name: "divide".to_string(),
            }],
            cache_output: false,
            has_cached_output: false,
            terminal: false,
        };

        let output = Node {
            id: output_id,
            name: "output".to_string(),
            pos: egui::pos2(800.0, 180.0),
            inputs: vec![Input {
                name: "value".to_string(),
                connection: Some(Connection {
                    node_id: divide_id,
                    output_index: 0,
                }),
            }],
            outputs: Vec::new(),
            cache_output: false,
            has_cached_output: false,
            terminal: true,
        };

        let graph = Self {
            id: Uuid::new_v4(),
            nodes: vec![value_a, value_b, sum, divide, output],
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            selected_node_id: None,
        };

        assert!(graph.nodes.len() == 5, "test_graph must contain 5 nodes");

        graph
    }

    pub fn select_node(&mut self, node_id: Uuid) {
        assert!(
            self.nodes.iter().any(|node| node.id == node_id),
            "selected node must exist in graph"
        );
        self.selected_node_id = Some(node_id);
    }

    pub fn remove_node(&mut self, node_id: Uuid) {
        assert!(
            self.nodes.iter().any(|node| node.id == node_id),
            "node must exist to be removed"
        );

        self.nodes.retain(|node| node.id != node_id);

        if self
            .selected_node_id
            .is_some_and(|selected| selected == node_id)
        {
            self.selected_node_id = None;
        }

        for node in &mut self.nodes {
            for input in &mut node.inputs {
                if let Some(connection) = &input.connection
                    && connection.node_id == node_id
                {
                    input.connection = None;
                }
            }
        }
    }
}

impl GraphFormat {
    pub fn from_extension(extension: &str) -> Result<Self> {
        let normalized = extension.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            bail!("graph file extension is empty");
        }

        match normalized.as_str() {
            "json" => Ok(Self::Json),
            "yaml" | "yml" => Ok(Self::Yaml),
            "toml" => Ok(Self::Toml),
            _ => bail!("unsupported graph file extension: {normalized}"),
        }
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow!("graph file extension is missing"))?;

        Self::from_extension(extension)
    }
}

#[test]
fn test_graph() {
    let graph = Graph::test_graph();
    assert!(graph.validate().is_ok());
}

#[test]
fn graph_roundtrip() {
    assert_roundtrip(GraphFormat::Json);
    assert_roundtrip(GraphFormat::Yaml);
    assert_roundtrip(GraphFormat::Toml);

    assert_file_roundtrip(GraphFormat::Json, "json");
    assert_file_roundtrip(GraphFormat::Yaml, "yaml");
    assert_file_roundtrip(GraphFormat::Toml, "toml");
}

fn assert_roundtrip(format: GraphFormat) {
    let graph = Graph::test_graph();
    let serialized = graph
        .serialize(format)
        .expect("graph serialization should succeed for test graph");
    assert!(
        !serialized.trim().is_empty(),
        "serialized graph should not be empty"
    );
    let deserialized = Graph::deserialize(format, &serialized)
        .expect("graph deserialization should succeed for test payload");
    assert!(deserialized.validate().is_ok());
    assert_eq!(
        graph.nodes.len(),
        deserialized.nodes.len(),
        "node counts should round-trip"
    );
    assert_eq!(
        graph.nodes[0].id, deserialized.nodes[0].id,
        "node ids should round-trip"
    );
    assert_eq!(graph.zoom, deserialized.zoom, "zoom should round-trip");
    assert_eq!(graph.pan, deserialized.pan, "pan should round-trip");
}

fn assert_file_roundtrip(format: GraphFormat, extension: &str) {
    let graph = Graph::test_graph();
    let detected =
        GraphFormat::from_extension(extension).expect("file extension must map to a graph format");
    assert_eq!(
        detected, format,
        "file extension must match the expected format"
    );
    let file_name = format!("egui-graph-{}.{}", Uuid::new_v4(), extension);
    let path = std::env::temp_dir().join(file_name);

    graph
        .serialize_to_file(&path)
        .expect("graph serialization to file should succeed");
    assert!(path.exists(), "serialized graph file should exist");

    let deserialized = Graph::deserialize_from_file(&path)
        .expect("graph deserialization from file should succeed");
    assert_eq!(
        graph.nodes.len(),
        deserialized.nodes.len(),
        "node counts should round-trip from file"
    );

    std::fs::remove_file(&path).expect("temporary graph file should be removable");
}
