using System.Diagnostics;

namespace csso.Graph.Tests;

public class FunctionTests {
    [SetUp]
    public void Setup() { }

    public static FunctionGraph CreateTestFunctionGraph() {
        var functionGraph = new csso.Graph.FunctionGraph();
        {
            var function = new Function();
            function.FromDelegate(() => 2);
            functionGraph.NewFunction(ref function);
            var node = new FunctionNode {
                FunctionIndex = function.SelfIndex,
                NodeIndex = 0
            };
            functionGraph.NewNode(ref node);
        }
        {
            var function = new Function();
            function.FromDelegate(() => 5);
            functionGraph.NewFunction(ref function);

            var node = new FunctionNode {
                FunctionIndex = function.SelfIndex,
                NodeIndex = 1
            };
            functionGraph.NewNode(ref node);
        }
        {
            var function = new Function();
            function.FromDelegate((int a, int b) => a + b);
            functionGraph.NewFunction(ref function);

            var node = new FunctionNode {
                FunctionIndex = function.SelfIndex,
                NodeIndex = 2
            };
            functionGraph.NewNode(ref node);
        }
        {
            var function = new Function();
            function.FromDelegate((int a, int b) => a * b);

            functionGraph.NewFunction(ref function);

            var node = new FunctionNode {
                FunctionIndex = function.SelfIndex,
                NodeIndex = 3
            };
            functionGraph.NewNode(ref node);
        }
        {
            var function = new Function();
            function.FromDelegate((int a) => { Debug.WriteLine("Print node: {0}", a); });
            functionGraph.NewFunction(ref function);

            var node = new FunctionNode {
                FunctionIndex = function.SelfIndex,
                NodeIndex = 4
            };
            functionGraph.NewNode(ref node);
        }

        return functionGraph;
    }

    [Test]
    public void Creation() {
        var graph = csso.Graph.Graph.FromJsonFile("./test_graph.json")!;

        var functionGraph = CreateTestFunctionGraph();
    }
}