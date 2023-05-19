using System.Diagnostics;

namespace csso.Graph.Tests;

public class FunctionTests {
    public const int Node1DefaultOutputValue = 5;
    public static int TestOutputValue;
    public static int Node1OutputValue = 5;

    public static readonly Delegate[] Delegates = {
        () => 2,
        () => Node1OutputValue,
        (int a, int b) => a + b,
        (int a, int b) => a * b,
        (int a) => {
            Debug.WriteLine("Print node: {0}", a);
            TestOutputValue = a;
        }
    };

    [SetUp]
    public void Setup() {
    }

    public static FunctionGraph CreateTestFunctionGraph() {
        Node1OutputValue = Node1DefaultOutputValue;
        TestOutputValue = 0;

        var functionGraph = new FunctionGraph();

        for (var i = 0; i < Delegates.Length; i++) {
            var function = Function.FromDelegate(Delegates[i]);
            function.NodeIndex = i;
            function.DelegateIndex = i;
            functionGraph.NewFunction(function);
        }

        return functionGraph;
    }

    [Test]
    public void Creation() {
        var graph = Graph.FromJsonFile("./test_graph.json")!;

        var functionGraph = CreateTestFunctionGraph();
    }
}