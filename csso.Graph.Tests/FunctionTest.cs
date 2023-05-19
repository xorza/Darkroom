using System.Diagnostics;

namespace csso.Graph.Tests;

public class FunctionTests {
    public static Int32 TestOutputValue = 0;

    [SetUp]
    public void Setup() { }

    public static FunctionGraph CreateTestFunctionGraph() {
        var functionGraph = new csso.Graph.FunctionGraph();
        {
            var function = Function.FromDelegate(() => 2);
            function.NodeIndex = 0;
            functionGraph.NewFunction(function);
        }
        {
            var function = Function.FromDelegate(() => 5);
            function.NodeIndex = 1;
            functionGraph.NewFunction(function);
        }
        {
            var function = Function.FromDelegate((int a, int b) => a + b);
            function.NodeIndex = 2;
            functionGraph.NewFunction(function);
        }
        {
            var function = Function.FromDelegate((int a, int b) => a * b);
            function.NodeIndex = 3;
            functionGraph.NewFunction(function);
        }
        {
            var function = Function.FromDelegate((int a) => {
                Debug.WriteLine("Print node: {0}", a);
                TestOutputValue = a;
            });
            function.NodeIndex = 4;
            functionGraph.NewFunction(function);
        }

        return functionGraph;
    }

    [Test]
    public void Creation() {
        var graph = csso.Graph.Graph.FromJsonFile("./test_graph.json")!;

        var functionGraph = CreateTestFunctionGraph();
    }
}