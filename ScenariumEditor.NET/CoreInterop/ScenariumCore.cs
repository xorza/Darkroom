using System.Diagnostics;
using System.Reflection;
using System.Reflection.Metadata;
using System.Runtime.InteropServices;


namespace CoreInterop;

public unsafe class ScenariumCore {
    public static void Init() {
        CoreNative.LoadDll();
    }

    private readonly byte* _ctx = null;

    public ScenariumCore() {
        if (!CoreNative.IsLoaded) {
            throw new InvalidOperationException("CoreNative is not loaded, call CoreNative.LoadDll() first.");
        }

        _ctx = CoreNative.create_context();
    }

    ~ScenariumCore() {
        CoreNative.destroy_context(_ctx);
    }

    public void GetNodes() {
        using var buf = CoreNative.get_nodes(_ctx);
        var nodes = buf.ToArray<FfiNode>();
        foreach (var node in nodes) {
            node.Dispose();
        }
    }

    public void GetFuncs() {
        using var buf = CoreNative.get_funcs(_ctx);

        var funcs = buf.ToArray<FfiFunc>();
        foreach (var func in funcs) {
            var func_id = func.id.ToGuid();
            Console.WriteLine($"Func: {func_id}, name: {func.name.ToString()}");

            var events = string.Join(", ", func.events.ToStringArray());
            Console.WriteLine($"Events: {events}");

            var new_node = CoreNative.new_node(_ctx, func.id);
            var node_id = new_node.id.ToGuid();
            Console.WriteLine($"Node: {node_id}, name: {new_node.name.ToString()}");

            new_node.Dispose();
            func.Dispose();
        }
    }
}