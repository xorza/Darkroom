using csso.Graph;

namespace csso.Nodeshop.Editor;

public class VisualNode {
    public Int32 NodeIndex { get; set; }
    public System.Drawing.Point Point { get; set; }
    public String Name { get; set; }
}

public class VisualGraph {
    public ExecutionGraph ExecutionGraph { get; set; }
    public List<VisualNode> Nodes { get; set; } = new();

    public static VisualGraph CreateTest() {
        var currentDirectory = Directory.GetCurrentDirectory();
        var graph = new VisualGraph();
        graph.ExecutionGraph = ExecutionGraph.FromJsonFile("../test_execution_graph.json");

        for (int i = 0; i < 5; i++) {
            var visualNode = new VisualNode {
                NodeIndex = i,
                Point = new System.Drawing.Point(i * 100, i * 100)
            };
            graph.Nodes.Add(visualNode);
        }

        return graph;
    }
}