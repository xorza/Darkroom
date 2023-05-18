namespace csso.Graph;

public struct IntermediateNode {
    public Int32 NodeIndex { get; set; }
    public NodeBehavior SelfBehavior { get; set; }
    public NodeBehavior ParentBehavior { get; set; }
    public bool IsComplete { get; set; }
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

        Int32 i = 0;
        while (i < activeNodes.Count) {
            var node = activeNodes[(int) i];
            ++i;

            var iNode = new IntermediateNode {
                NodeIndex = node.SelfIndex,
                SelfBehavior = node.Behavior,
                ParentBehavior = NodeBehavior.Passive,
                IsComplete = true
            };


            var inputs = graph.InputsForNode(node.SelfIndex);
            foreach (Input input in inputs) {
                var edge = graph.EdgeForInput(input.SelfIndex);
                if (edge != null) {
                    var output = graph.Outputs[(int) edge.Value.OutputIndex];
                    var newNode = graph.Nodes[(int) output.NodeIndex];
                    activeNodes.Add(newNode);
                } else {
                    iNode.IsComplete = false;
                }
            }

            Nodes.Add(iNode);
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
                    var output = graph.Outputs[(int) edge.Value.OutputIndex];
                    var outputINode = Nodes.First(_ => _.NodeIndex == output.NodeIndex);
                    if (outputINode.SelfBehavior == NodeBehavior.Active
                        || outputINode.ParentBehavior == NodeBehavior.Active) {
                        iIntermediateNode.ParentBehavior = NodeBehavior.Active;
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