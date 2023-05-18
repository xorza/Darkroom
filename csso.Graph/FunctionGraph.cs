using System.ComponentModel;

namespace csso.Graph;

public enum Direction {
    In,
    Out
}

public struct Argument {
    public Type Type { get; set; }
    public String Name { get; set; }
    public Direction Direction { get; set; }
    public bool IsRequired { get; set; }
}

public struct Function {
    public Int32 SelfIndex { get; set; }

    public Delegate @Delegate { get; set; }

    public String Name { get; set; }

    public List<Argument> Arguments { get; set; }

    public void FromDelegate(Delegate @delegate) {
        var method = @delegate.Method;
        var parameters = method.GetParameters();
        var returnType = method.ReturnType;

        @Delegate = @delegate;
        Name = @delegate.Method.Name;
        Arguments = new List<Argument>();

        var returnArg = new Argument();
        returnArg.Direction = Direction.Out;
        returnArg.Type = returnType;
        returnArg.Name = "return";
        Arguments.Add(returnArg);

        foreach (var parameter in parameters) {
            var arg = new Argument();
            arg.Type = parameter.ParameterType;
            arg.Name = parameter.Name ?? throw new InvalidEnumArgumentException();

            if (parameter.IsOut) {
                arg.Direction = Direction.Out;
                Arguments.Add(arg);
            } else {
                arg.Direction = Direction.In;
            }

            Arguments.Add(arg);
        }

        ;
    }

    public void Invoke(object?[]? parameters = null) {
        Delegate.DynamicInvoke(parameters);
    }
}

public struct FunctionNode {
    public Int32 SelfIndex { get; set; }
    public Int32 NodeIndex { get; set; }
    public Int32 FunctionIndex { get; set; }
}

public class FunctionGraph {
    public List<Function> Functions { get; } = new();
    public List<FunctionNode> Nodes { get; } = new();

    public void NewFunction(ref Function function) {
        function.SelfIndex = Functions.Count;
        Functions.Add(function);
    }

    public void NewNode(ref FunctionNode node) {
        node.SelfIndex = Nodes.Count;
        Nodes.Add(node);
    }
}