using System.Diagnostics;
using System.Reflection;
using System.Reflection.Metadata;
using System.Runtime.InteropServices;


namespace CoreInterop;

public unsafe class ScenariumCore {
    public static void Init() {
        CoreNative.LoadDll();
    }

    private readonly byte* _ctx;

    public ScenariumCore() {
        if (!CoreNative.IsLoaded) {
            throw new InvalidOperationException("CoreNative is not loaded, call CoreNative.LoadDll() first.");
        }

        _ctx = CoreNative.create_context();
    }

    ~ScenariumCore() {
        CoreNative.destroy_context(_ctx);
    }
}