using System.Diagnostics;
using System.Text.Json;
using csso.Common;

namespace csso.Graph;

public class ExecutionNode {
    public int NodeIndex { get; set; }
    public object?[]? Arguments { get; set; }
    public object? Return { get; set; }

    public bool IsExecuted { get; set; }
    public bool HasOutputs { get; set; }
    public TimeSpan ExecutionTime { get; set; }
}

public struct ExecutionArgument {
    public int InputIndex { get; set; }
    public int OutputIndex { get; set; }
    public int FunctionIndex { get; set; }
    public int ArgumentIndex { get; set; }
}

public class ExecutionCache {
    public List<ExecutionNode> Nodes { get; set; } = new();
}

public class ExecutionContext {
    public List<Delegate> Delegates { get; set; } = new();
}

public class ExecutionGraph {
    public FunctionGraph FunctionGraph { get; set; } = new();
    public Graph Graph { get; set; } = new();
    public List<ExecutionArgument> Arguments { get; set; } = new();


    public string ToJson() {
        var jsonString = JsonSerializer.Serialize(this);
        return jsonString;
    }

    public static ExecutionGraph FromJsonFile(string filename) {
        var jsonString = File.ReadAllText(filename);
        var executionGraph = JsonSerializer.Deserialize<ExecutionGraph>(jsonString);
        return executionGraph!;
    }

    public ExecutionCache Run(ExecutionContext context, ExecutionCache previousCache) {
        ExecutionCache currentExecution = new();

        var intermediateGraph = new IntermediateGraph(Graph);
        foreach (var iNode in intermediateGraph.Nodes) {
            var function = FunctionGraph.Functions.Single(_ => _.NodeIndex == iNode.NodeIndex);
            var @delegate = context.Delegates[function.DelegateIndex];
            var executionNode = MakeNode(function.NodeIndex, previousCache);

            currentExecution.Nodes.Add(executionNode);

            if (iNode.IsComplete == false)
                continue;

            if (executionNode.HasOutputs) {
                if (iNode.EdgeBehavior == EdgeBehavior.Once)
                    continue;

                if (iNode.SelfBehavior == NodeBehavior.Passive) {
                    var hasUpdatedInputs = false;
                    foreach (var input in Graph.InputsForNode(iNode.NodeIndex)) {
                        var edge = Graph.EdgeForInput(input.SelfIndex);
                        if (edge is null) {
                            Debug.Assert(input.Required == false);
                        }
                        else {
                            if (edge.Behavior == EdgeBehavior.Always) {
                                var output = Graph.Outputs[edge.OutputIndex];
                                var outputExecutionNode =
                                    currentExecution.Nodes.Single(_ => _.NodeIndex == output.NodeIndex);

                                if (outputExecutionNode.IsExecuted) hasUpdatedInputs = true;
                            }
                        }
                    }

                    if (!hasUpdatedInputs) continue;
                }
            }

            if (MyDebug.IsDebug) {
                var node = Graph.Nodes[iNode.NodeIndex];
                Debug.Assert(function.Arguments.Count
                             == Graph.Inputs.Count(_ => _.NodeIndex == node.SelfIndex)
                             + Graph.Outputs.Count(_ => _.NodeIndex == node.SelfIndex));
                Debug.Assert(@delegate.Method.GetParameters().Length == function.InvokeArgumentsCount);
            }

            foreach (var argument in function.Arguments)
                ProcessArgument(function, executionNode, argument, currentExecution);


            var timer = Stopwatch.StartNew();
            executionNode.Return = @delegate.DynamicInvoke(executionNode.Arguments);
            timer.Stop();

            executionNode.ExecutionTime = timer.Elapsed;
            executionNode.HasOutputs = true;
            executionNode.IsExecuted = true;
        }

        return currentExecution;
    }

    private ExecutionNode MakeNode(int nodeIndex, ExecutionCache cache) {
        var result = new ExecutionNode {
            NodeIndex = nodeIndex,
            Arguments = null,
            HasOutputs = false
        };

        var function = FunctionGraph.Functions.Single(_ => _.NodeIndex == nodeIndex);

        var executionNode = cache.Nodes.SingleOrDefault(_ => _.NodeIndex == nodeIndex);
        if (executionNode is { HasOutputs: true }) {
            result.Arguments = executionNode.Arguments;
            result.Return = executionNode.Return;
            result.HasOutputs = true;
        }
        else if (function.InvokeArgumentsCount > 0) {
            result.Arguments = new object?[function.InvokeArgumentsCount];
        }

        return result;
    }

    private void ProcessArgument(
        Function function,
        ExecutionNode executionNode,
        Argument argument,
        ExecutionCache currentExecution) {
        if (argument.Direction == Direction.In) {
            var executionArgument = Arguments.Single(_ =>
                _.ArgumentIndex == argument.SelfIndex && _.FunctionIndex == function.SelfIndex);
            var input = Graph.Inputs[executionArgument.InputIndex];
            var edge = Graph.EdgeForInput(input.SelfIndex)!;
            var output = Graph.Outputs[edge.OutputIndex];
            var outputFunction = FunctionGraph.Functions.Single(_ => _.NodeIndex == output.NodeIndex);
            var outputExecutionNode = currentExecution.Nodes.Single(_ => _.NodeIndex == output.NodeIndex);
            var outputExecutionArgument = Arguments.Single(_ =>
                _.OutputIndex == output.SelfIndex && _.FunctionIndex == outputFunction.SelfIndex);
            var outputArgument = outputFunction.Arguments[outputExecutionArgument.ArgumentIndex];

            Debug.Assert(outputExecutionNode.HasOutputs);

            var value = outputArgument.Position == -1
                ? outputExecutionNode.Return
                : outputExecutionNode.Arguments![outputArgument.Position];

            executionNode.Arguments![argument.Position] = value;
        }
        else {
            if (argument.Position > 0) executionNode.Arguments![argument.Position] = null;
        }
    }
}