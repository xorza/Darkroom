using System.Diagnostics;
using csso.Common;

namespace csso.Graph;

public class ExecutionNode {
    public Int32 NodeIndex { get; set; }
    public Object?[]? Arguments { get; set; } = null;
    public Object? Return { get; set; } = null;

    public bool IsExecuted { get; set; } = false;
    public bool HasOutputs { get; set; } = false;
    public TimeSpan ExecutionTime { get; set; }
}

public struct ExecutionArgument {
    public Int32 InputIndex { get; set; }
    public Int32 OutputIndex { get; set; }
    public Int32 FunctionIndex { get; set; }
    public Int32 ArgumentIndex { get; set; }
}

public class ExecutionGraph {
    public FunctionGraph FunctionGraph { get; set; } = new();
    public Graph Graph { get; set; } = new();

    public List<ExecutionNode> CurrentRunNodes { get; set; } = new();
    public List<ExecutionArgument> Arguments { get; set; } = new();

    public List<ExecutionNode> LastRunNodes { get; set; } = new();

    public void Run() {
        var intermediateGraph = new IntermediateGraph(Graph);
        foreach (IntermediateNode intermediateNode in intermediateGraph.Nodes) {
            var function = FunctionGraph.Functions.Single(_ => _.NodeIndex == intermediateNode.NodeIndex);
            var executionNode = GetOrCreateNode(function.NodeIndex);

            CurrentRunNodes.Add(executionNode);

            if (MyDebug.IsDebug) {
                var node = Graph.Nodes[intermediateNode.NodeIndex];
                Debug.Assert(function.Arguments.Count
                             == Graph.Inputs.Count(_ => _.NodeIndex == node.SelfIndex)
                             + Graph.Outputs.Count(_ => _.NodeIndex == node.SelfIndex));
            }

            foreach (var argument in function.Arguments) {
                ProcessArgument(function, executionNode, argument);
            }

            var timer = Stopwatch.StartNew();
            executionNode.Return = function.Invoke(executionNode.Arguments);
            timer.Stop();

            executionNode.ExecutionTime = timer.Elapsed;
            executionNode.HasOutputs = true;
            executionNode.IsExecuted = true;
        }

        LastRunNodes = CurrentRunNodes;
    }

    private ExecutionNode GetOrCreateNode(Int32 nodeIndex) {
        ExecutionNode? executionNode = LastRunNodes.SingleOrDefault(_ => _.NodeIndex == nodeIndex);
        if (executionNode == null) {
            executionNode = new ExecutionNode {
                NodeIndex = nodeIndex
            };
            var function = FunctionGraph.Functions.Single(_ => _.NodeIndex == nodeIndex);
            if (function.InvokeArgumentsCount > 0) {
                executionNode.Arguments = new Object?[function.InvokeArgumentsCount];
            }
        }

        return executionNode;
    }

    private void ProcessArgument(
        Function function,
        ExecutionNode executionNode,
        Argument argument) {
        if (argument.Direction == Direction.In) {
            var executionArgument = Arguments.Single(_ =>
                _.ArgumentIndex == argument.SelfIndex && _.FunctionIndex == function.SelfIndex);
            var input = Graph.Inputs[executionArgument.InputIndex];
            var edge = Graph.EdgeForInput(input.SelfIndex)!.Value;
            var output = Graph.Outputs[edge.OutputIndex];
            var outputFunction = FunctionGraph.Functions.Single(_ => _.NodeIndex == output.NodeIndex);
            var outputExecutionNode = CurrentRunNodes.Single(_ => _.NodeIndex == output.NodeIndex);
            var outputExecutionArgument = Arguments.Single(_ =>
                _.OutputIndex == output.SelfIndex && _.FunctionIndex == outputFunction.SelfIndex);
            var outputArgument = outputFunction.Arguments[outputExecutionArgument.ArgumentIndex];

            Debug.Assert(outputExecutionNode.HasOutputs);

            Object? value = outputArgument.Position == -1
                ? outputExecutionNode.Return
                : outputExecutionNode.Arguments![outputArgument.Position];

            executionNode.Arguments![argument.Position] = value;
        } else {
            if (argument.Position > 0) {
                executionNode.Arguments![argument.Position] = null;
            }
        }
    }
}