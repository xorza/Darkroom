using System.Runtime.InteropServices;
using YamlDotNet.Core;
using YamlDotNet.Core.Events;
using YamlDotNet.Serialization;

namespace CoreInterop.Utils;

public readonly struct Uuid : IEquatable<Uuid> {
    private readonly UInt64 _a = 0;
    private readonly UInt64 _b = 0;


    [DllImport(ScenariumCore.DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern Uuid uuid_new_v4_extern();

    [DllImport(ScenariumCore.DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern FfiBuf uuid_to_string_extern(Uuid uuid);

    [DllImport(ScenariumCore.DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern Uuid uuid_from_string_extern(FfiBuf buf);

    public Uuid() {
    }

    public static Uuid NewV4() {
        return uuid_new_v4_extern();
    }

    public static Uuid FromString(String s) {
        using var buf = new FfiBuf(s);
        return uuid_from_string_extern(buf);
    }

    public override String ToString() {
        return uuid_to_string_extern(this).ToString();
    }

    public override bool Equals(object obj) {
        return obj is Uuid other && Equals(other);
    }

    public bool Equals(Uuid other) {
        return _a == other._a && _b == other._b;
    }

    public override int GetHashCode() {
        return HashCode.Combine(_a, _b);
    }
}

public class UuidConverter : IYamlTypeConverter {
    public bool Accepts(Type type) {
        return type == typeof(Uuid);
    }

    public object ReadYaml(IParser parser, Type type) {
        var scalar = parser.Consume<Scalar>();
        return Uuid.FromString(scalar.Value);
    }

    public void WriteYaml(IEmitter emitter, object value, Type type) {
        var uuid = (Uuid)value!;
        emitter.Emit(new Scalar(uuid.ToString()));
    }
}