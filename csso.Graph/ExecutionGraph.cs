using System.Diagnostics;
using System.Text.Json;
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


    public String ToJson() {
        string jsonString = JsonSerializer.Serialize(this);
        return jsonString;
    }

    public static ExecutionGraph FromJsonFile(String filename) {
        string jsonString = File.ReadAllText(filename);
        var executionGraph = JsonSerializer.Deserialize<ExecutionGraph>(jsonString);
        return executionGraph!;
    }

    public ExecutionCache Run(ExecutionContext context, ExecutionCache previousCache) {
        ExecutionCache currentExecution = new();

        var intermediateGraph = new IntermediateGraph(Graph);
        foreach (IntermediateNode iNode in intermediateGraph.Nodes) {
            var function = FunctionGraph.Functions.Single(_ => _.NodeIndex == iNode.NodeIndex);
            var @delegate = context.Delegates[function.DelegateIndex];
            var executionNode = MakeNode(function.NodeIndex, previousCache);

            currentExecution.Nodes.Add(executionNode);

            if (executionNode.HasOutputs) {
                if (iNode.EdgeBehavior == EdgeBehavior.Once) {
                    continue;
                }

                if (iNode.SelfBehavior == NodeBehavior.Passive) {
                    continue;
                }
            }

            if (MyDebug.IsDebug) {
                var node = Graph.Nodes[iNode.NodeIndex];
                Debug.Assert(function.Arguments.Count
                             == Graph.Inputs.Count(_ => _.NodeIndex == node.SelfIndex)
                             + Graph.Outputs.Count(_ => _.NodeIndex == node.SelfIndex));
                Debug.Assert(@delegate.Method.GetParameters().Length == function.InvokeArgumentsCount);
            }

            foreach (var argument in function.Arguments) {
                ProcessArgument(function, executionNode, argument, currentExecution);
            }


            var timer = Stopwatch.StartNew();
            executionNode.Return = @delegate.DynamicInvoke(executionNode.Arguments);
            timer.Stop();

            executionNode.ExecutionTime = timer.Elapsed;
            executionNode.HasOutputs = true;
            executionNode.IsExecuted = true;
        }

        return currentExecution;
    }

    private ExecutionNode MakeNode(Int32 nodeIndex, ExecutionCache cache) {
        var result = new ExecutionNode() {
            NodeIndex = nodeIndex,
            Arguments = null,
            HasOutputs = false
        };

        var function = FunctionGraph.Functions.Single(_ => _.NodeIndex == nodeIndex);

        ExecutionNode? executionNode = cache.Nodes.SingleOrDefault(_ => _.NodeIndex == nodeIndex);
        if (executionNode is {HasOutputs: true}) {
            result.Arguments = executionNode.Arguments;
            result.Return = executionNode.Return;
            result.HasOutputs = true;
        } else if (function.InvokeArgumentsCount > 0) {
            result.Arguments = new Object?[function.InvokeArgumentsCount];
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
            var edge = Graph.EdgeForInput(input.SelfIndex)!.Value;
            var output = Graph.Outputs[edge.OutputIndex];
            var outputFunction = FunctionGraph.Functions.Single(_ => _.NodeIndex == output.NodeIndex);
            var outputExecutionNode = currentExecution.Nodes.Single(_ => _.NodeIndex == output.NodeIndex);
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