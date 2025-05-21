using System.Diagnostics;
using YamlDotNet.Core;
using YamlDotNet.Core.Events;
using YamlDotNet.Serialization;

namespace CoreInterop.Utils;

public enum DataTypes {
    F64,
    I64,
    String,
    Custom
}

public abstract class DataType {
    public abstract DataTypes Type { get; }
}

public class F64DataType : DataType {
    public override DataTypes Type => DataTypes.F64;
}

public class I64DataType : DataType {
    public override DataTypes Type => DataTypes.I64;
}

public class StringDataType : DataType {
    public override DataTypes Type => DataTypes.String;
}

public class CustomDataType : DataType {
    public override DataTypes Type => DataTypes.Custom;
}

public class DataTypeConverter : IYamlTypeConverter {
    public bool Accepts(Type type) {
        return type == typeof(DataType);
    }

    public static DataType FromString(String s) {
        switch (s) {
            case "Float":
                return new F64DataType();
            case "Int":
                return new I64DataType();
            case "String":
                return new StringDataType();

            default:
                throw new NotImplementedException(s);
        }
    }

    public object ReadYaml(IParser parser, Type type) {
        var scalar = parser.Consume<Scalar>();

        return FromString(scalar.Value);
    }

    public void WriteYaml(IEmitter emitter, object value, Type type) {
        throw new NotImplementedException();
    }
}