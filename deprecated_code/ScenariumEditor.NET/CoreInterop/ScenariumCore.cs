using System.Diagnostics;
using System.Runtime.InteropServices;
using CoreInterop.Utils;
using YamlDotNet.Serialization;
using YamlDotNet.Serialization.NamingConventions;

namespace CoreInterop;

public partial class ScenariumCore : IDisposable {
    public enum CallbackType : UInt32 {
        OnGraphUpdate,
        OnFuncLibUpdate,
    }
    
    public class CallbackEventArgs : EventArgs {
        public ScenariumCore Core { get; set; }
        public CallbackType Type { get; set; }
    }

    public event EventHandler<EventArgs>? CallbackEvent;


    private IntPtr _ctx = IntPtr.Zero;

    public ScenariumCore() {
        _ctx = LibraryLoader.create_context();
        LibraryLoader.register_callback(_ctx, (value) => {
            CallbackEvent?.Invoke(this, new CallbackEventArgs {
                Core = this,
                Type = value
            });
        });
    }

    ~ScenariumCore() {
        ReleaseUnmanagedResources();
    }

    private readonly IDeserializer _deserializer = new DeserializerBuilder()
        .WithTagMapping("!Float", typeof(Value))
        .WithTagMapping("!Int", typeof(Value))
        .WithTagMapping("!Output", typeof(OutputBinding))
        .WithTagMapping("!Const", typeof(ConstBinding))
        .WithTypeConverter(new UuidConverter())
        .WithTypeConverter(new DataTypeConverter())
        .WithTypeConverter(new ValueConverter())
        .Build();


    public Graph GetGraph() {
        using FfiBuf buf = LibraryLoader.GetGraph(_ctx);
        var yaml = buf.ToString();

        return _deserializer.Deserialize<Graph>(new StringReader(yaml));
    }

    public FuncLib GetFuncLib() {
        using FfiBuf buf = LibraryLoader.GetFuncLib(_ctx);
        var yaml = buf.ToString();

        var funcs = _deserializer.Deserialize<List<Func>>(new StringReader(yaml));
        return new FuncLib() {
            Funcs = funcs
        };
    }

    private void ReleaseUnmanagedResources() {
        if (_ctx != IntPtr.Zero)
            LibraryLoader.destroy_context(_ctx);
        _ctx = IntPtr.Zero;
    }

    public void Dispose() {
        ReleaseUnmanagedResources();
        GC.SuppressFinalize(this);
    }
}