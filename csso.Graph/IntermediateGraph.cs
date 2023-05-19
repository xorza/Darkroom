namespace csso.Graph;

public class IntermediateNode {
    public Int32 NodeIndex { get; set; }
    public NodeBehavior SelfBehavior { get; set; }
    public bool IsComplete { get; set; }
    public EdgeBehavior EdgeBehavior { get; set; }
}

public class IntermediateGraph {
    public IntermediateGraph(Graph graph) {
        Graph = graph;

        TraverseBackward(graph);
        TraverseForward(graph);
    }

    public List<IntermediateNode> Nodes { get; private set; } = new();
    public Graph Graph { get; init; }

    private void TraverseBackward(Graph graph) {
        var activeNodes = graph.Nodes.Where(_ => _.IsOutput).ToList();

        Nodes.Clear();
        foreach (var node in activeNodes) {
            var iNode = new IntermediateNode {
                NodeIndex = node.SelfIndex,
                SelfBehavior = NodeBehavior.Active,
                EdgeBehavior = EdgeBehavior.Always,
                IsComplete = true
            };
            Nodes.Add(iNode);
        }

        Int32 i = 0;
        while (i < Nodes.Count) {
            var iNode = Nodes[i];
            ++i;

            var inputs = graph.InputsForNode(iNode.NodeIndex);
            foreach (Input input in inputs) {
                var edge = graph.EdgeForInput(input.SelfIndex);
                if (edge != null) {
                    var output = graph.Outputs[edge.Value.OutputIndex];
                    var outputNode = graph.Nodes[output.NodeIndex];

                    IntermediateNode iOutputNode;
                    if (Nodes.SingleOrDefault(_ => _.NodeIndex == outputNode.SelfIndex) is IntermediateNode node) {
                        iOutputNode = node;
                    } else {
                        iOutputNode = new IntermediateNode {
                            NodeIndex = outputNode.SelfIndex,
                            SelfBehavior = outputNode.Behavior,
                            IsComplete = true,
                            EdgeBehavior = EdgeBehavior.Once
                        };
                        Nodes.Add(iOutputNode);
                    }

                    if (iNode.EdgeBehavior == EdgeBehavior.Always
                        && edge.Value.Behavior == EdgeBehavior.Always) {
                        iOutputNode.EdgeBehavior = EdgeBehavior.Always;
                    }
                } else {
                    iNode.IsComplete = false;
                }
            }
        }

        Nodes.Reverse();
    }

    private void TraverseForward(Graph graph) {
        for (int i = 0; i < Nodes.Count; i++) {
            IntermediateNode iIntermediateNode = Nodes[i];
            var inputs = graph.InputsForNode(iIntermediateNode.NodeIndex);
            foreach (Input input in inputs) {
                var edge = graph.EdgeForInput(input.SelfIndex);
                if (edge != null) {
                    var output = graph.Outputs[edge.Value.OutputIndex];
                    var outputINode = Nodes.First(_ => _.NodeIndex == output.NodeIndex);
                    if (outputINode.SelfBehavior == NodeBehavior.Active) {
                        iIntermediateNode.SelfBehavior = NodeBehavior.Active;
                    }

                    if (outputINode.IsComplete == false) {
                        iIntermediateNode.IsComplete = false;
                    }
                } else {
                    if (input.Required) {
                        iIntermediateNode.IsComplete = false;
                    }
                }
            }

            Nodes[i] = iIntermediateNode;
        }


        Nodes = Nodes.Distinct().ToList();
    }
}