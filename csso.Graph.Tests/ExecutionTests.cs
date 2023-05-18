namespace csso.Graph.Tests;

public class ExecutionTests {
    [SetUp]
    public void Setup() { }

    [Test]
    public void Creation() {
        var functionGraph = FunctionTests.CreateTestFunctionGraph();
        var graph = GraphTests.CreateTestGraph();
        var executionGraph = new ExecutionGraph {
            FunctionGraph = functionGraph,
            Graph = graph
        };

        executionGraph.Run();

        Assert.Pass();
    }
}