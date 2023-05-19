namespace csso.Graph.Tests;

public class GraphTests {
    [SetUp]
    public void Setup() { }

    public static Graph CreateTestGraph() {
        var graph = csso.Graph.Graph.FromJsonFile("./test_graph.json")!;
        return graph;
    }

    [Test]
    public void Creation() {
        var node = new Node();
        node.Name = "test";

        var graph = new csso.Graph.Graph();
        graph.NewNode(ref node);

        Assert.Pass();
    }

    [Test]
    public void ToJson() {
        var node = new Node {
            Name = "test"
        };

        var graph = new csso.Graph.Graph();
        graph.NewNode(ref node);

        String json = graph.ToJson();

        Assert.Pass();
    }

    [Test]
    public void GraphFromJson() {
        var graph = CreateTestGraph();

        Assert.That(graph.Nodes.Count, Is.EqualTo(5));
        Assert.That(graph.Inputs.Count, Is.EqualTo(5));
        Assert.That(graph.Outputs.Count, Is.EqualTo(4));
        Assert.That(graph.Edges.Count, Is.EqualTo(5));


        Assert.Pass();
    }

    [Test]
    public void EdgeReplacement() {
        var graph = CreateTestGraph();
        var edge = new Edge {
            OutputIndex = 1,
            InputIndex = 4
        };

        graph.NewEdge(ref edge);

        Assert.That(edge.SelfIndex, Is.EqualTo(3));

        var input = graph.Inputs[4];
        Assert.That(graph.Edges.Count(_ => _.InputIndex == input.SelfIndex), Is.EqualTo(1));
        Assert.That(graph.Edges.Count, Is.EqualTo(5));
    }

    [Test]
    public void GraphPreprocessing() {
        var graph = CreateTestGraph();
        var intermediateGraph = new csso.Graph.IntermediateGraph(graph);

        Assert.That(intermediateGraph.Nodes.Count, Is.EqualTo(5));

        var iNode4 = intermediateGraph.Nodes[4];
        Assert.That(graph.Nodes[iNode4.NodeIndex].Name, Is.EqualTo("print node"));
        Assert.That(iNode4.IsComplete, Is.EqualTo(true));
        Assert.That(iNode4.SelfBehavior, Is.EqualTo(NodeBehavior.Active));

        var iNode3 = intermediateGraph.Nodes[3];
        Assert.That(graph.Nodes[iNode3.NodeIndex].Name, Is.EqualTo("mult node"));
        Assert.That(iNode3.IsComplete, Is.EqualTo(true));
        Assert.That(iNode3.SelfBehavior, Is.EqualTo(NodeBehavior.Passive));

        var iNode1 = intermediateGraph.Nodes[1];
        Assert.That(graph.Nodes[iNode1.NodeIndex].Name, Is.EqualTo("value 1 node"));
        Assert.That(iNode1.IsComplete, Is.EqualTo(true));
        Assert.That(iNode1.SelfBehavior, Is.EqualTo(NodeBehavior.Passive));

        var iNode0 = intermediateGraph.Nodes[0];
        Assert.That(graph.Nodes[iNode0.NodeIndex].Name, Is.EqualTo("value 0 node"));
        Assert.That(iNode0.IsComplete, Is.EqualTo(true));
        Assert.That(iNode0.SelfBehavior, Is.EqualTo(NodeBehavior.Active));
    }
}