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

        executionGraph.Arguments.Add(new ExecutionArgument {
            InputIndex = 0,
            OutputIndex = 0,
            FunctionIndex = 0,
            ArgumentIndex = 0
        });
        executionGraph.Arguments.Add(new ExecutionArgument {
            InputIndex = 0,
            OutputIndex = 1,
            FunctionIndex = 1,
            ArgumentIndex = 0
        });

        {
            //sum
            executionGraph.Arguments.Add(new ExecutionArgument {
                InputIndex = 0,
                OutputIndex = 0,
                FunctionIndex = 2,
                ArgumentIndex = 0
            });
            executionGraph.Arguments.Add(new ExecutionArgument {
                InputIndex = 1,
                OutputIndex = 0,
                FunctionIndex = 2,
                ArgumentIndex = 1
            });
            executionGraph.Arguments.Add(new ExecutionArgument {
                InputIndex = 0,
                OutputIndex = 2,
                FunctionIndex = 2,
                ArgumentIndex = 2
            });
        }

        {
            //mult
            executionGraph.Arguments.Add(new ExecutionArgument {
                InputIndex = 2,
                OutputIndex = 0,
                FunctionIndex = 3,
                ArgumentIndex = 0
            });
            executionGraph.Arguments.Add(new ExecutionArgument {
                InputIndex = 3,
                OutputIndex = 0,
                FunctionIndex = 3,
                ArgumentIndex = 1
            });
            executionGraph.Arguments.Add(new ExecutionArgument {
                InputIndex = 0,
                OutputIndex = 3,
                FunctionIndex = 3,
                ArgumentIndex = 2
            });
        }

        executionGraph.Arguments.Add(new ExecutionArgument {
            InputIndex = 4,
            OutputIndex = 0,
            FunctionIndex = 4,
            ArgumentIndex = 0
        });


        FunctionTests.TestOutputValue = 0;
        executionGraph.Run();

        Assert.That(FunctionTests.TestOutputValue, Is.EqualTo(35));
        Assert.Pass();
    }
}