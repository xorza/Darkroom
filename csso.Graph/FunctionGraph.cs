using System.ComponentModel;

namespace csso.Graph;

public enum Direction {
    In,
    Out
}

public class Argument {
    public int SelfIndex { get; set; }
    public DataType Type { get; set; }
    public required string Name { get; set; }
    public Direction Direction { get; set; }
    public bool IsRequired { get; set; } = true;

    public int Position { get; set; }
}

public class Function {
    public int SelfIndex { get; set; }
    public int NodeIndex { get; set; }

    public required string Name { get; set; }

    public required List<Argument> Arguments { get; set; }
    public int InvokeArgumentsCount { get; set; }
    public int DelegateIndex { get; set; }

    public static Function FromDelegate(Delegate @delegate) {
        var result = new Function {
            Name = @delegate.Method.Name,
            Arguments = new List<Argument>()
        };

        foreach (var parameter in @delegate.Method.GetParameters()) {
            var arg = new Argument {
                Type = DataType.FromType(parameter.ParameterType),
                Name = parameter.Name ?? throw new InvalidEnumArgumentException(),
                Direction = parameter.IsOut ? Direction.Out : Direction.In,
                IsRequired = !parameter.IsOptional,
                SelfIndex = result.Arguments.Count,
                Position = parameter.Position
            };

            result.Arguments.Add(arg);
        }

        result.InvokeArgumentsCount = result.Arguments.Count;

        if (@delegate.Method.ReturnType != typeof(void)) {
            var returnArg = new Argument {
                Direction = Direction.Out,
                Type = DataType.FromType(@delegate.Method.ReturnType),
                Name = "return",
                SelfIndex = result.Arguments.Count,
                Position = -1
            };
            result.Arguments.Add(returnArg);
        }

        return result;
    }
}

public class FunctionGraph {
    public List<Function> Functions { get; set; } = new();


    public void NewFunction(Function function) {
        function.SelfIndex = Functions.Count;
        Functions.Add(function);
    }
}