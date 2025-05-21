using System.Globalization;
using YamlDotNet.Core;
using YamlDotNet.Core.Events;
using YamlDotNet.Serialization;

namespace CoreInterop.Utils;

public class Value {
    public DataType Type { get; set; }

    private Object _data = null;

    public Object Data {
        get => _data;
        set { _data = value; }
    }

    public Value(DataType type) {
        Type = type;
    }

    public override string ToString() {
        return _data.ToString();
    }

    public void FromString(String s) {
        switch (Type.Type) {
            case DataTypes.F64:
                Data = double.Parse(s, NumberStyles.Any, CultureInfo.InvariantCulture);
                break;
            case DataTypes.I64:
                Data = long.Parse(s, NumberStyles.Any, CultureInfo.InvariantCulture);
                break;
            case DataTypes.String:
                Data = s;
                break;
            default:
                throw new NotImplementedException();
        }
    }
}

public class ValueConverter : IYamlTypeConverter {
    public bool Accepts(Type type) {
        return type == typeof(Value);
    }

    public object ReadYaml(IParser parser, Type type) {
        var scalar = parser.Consume<Scalar>();
        var tag = scalar.Tag.Value.TrimStart('!');
        var data_type = DataTypeConverter.FromString(tag);
        
        var value = new Value(data_type);
        value.FromString(scalar.Value);

        return value;
    }

    public void WriteYaml(IEmitter emitter, object value, Type type) {
        throw new NotImplementedException();
    }
}