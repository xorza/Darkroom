namespace csso.Graph;

public class ExecutionGraph {
    public FunctionGraph FunctionGraph { get; set; } = new();
    public Graph Graph { get; set; } = new();

    public void Run() {
        var intermediateGraph = new IntermediateGraph(Graph);
        foreach (IntermediateNode intermediateNode in intermediateGraph.Nodes) {
            var functionNode = FunctionGraph.Nodes.Single(_ => _.NodeIndex == intermediateNode.NodeIndex);
            var function = FunctionGraph.Functions[functionNode.FunctionIndex];
            var node = Graph.Nodes[functionNode.NodeIndex];
            function.Invoke();
        }
    }
}