using System.Runtime.InteropServices;

namespace CoreInterop.Utils;

internal static partial class LibraryLoader {
    private static readonly string DllName =
        OperatingSystem.IsWindows() ? "core_interop.dll" : "libcore_interop.dylib";

    // Windows P/Invoke
    [LibraryImport("kernel32.dll", EntryPoint = "LoadLibraryA", SetLastError = true,
        StringMarshalling = StringMarshalling.Custom,
        StringMarshallingCustomType = typeof(System.Runtime.InteropServices.Marshalling.AnsiStringMarshaller))]
    private static partial IntPtr LoadLibrary(string lpFileName);

    [LibraryImport("kernel32.dll", EntryPoint = "GetProcAddress", SetLastError = true,
        StringMarshalling = StringMarshalling.Custom,
        StringMarshallingCustomType = typeof(System.Runtime.InteropServices.Marshalling.AnsiStringMarshaller))]
    private static partial IntPtr GetProcAddress(IntPtr hModule, string lpProcName);

    [LibraryImport("kernel32.dll", EntryPoint = "FreeLibrary", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool FreeLibrary(IntPtr hModule);

    // macOS/Linux P/Invoke
    [LibraryImport("libdl.dylib", EntryPoint = "dlopen", SetLastError = true,
        StringMarshalling = StringMarshalling.Custom,
        StringMarshallingCustomType = typeof(System.Runtime.InteropServices.Marshalling.AnsiStringMarshaller))]
    private static partial IntPtr Dlopen(string filename, int flags);

    [LibraryImport("libdl.dylib", EntryPoint = "dlsym", SetLastError = true,
        StringMarshalling = StringMarshalling.Custom,
        StringMarshallingCustomType = typeof(System.Runtime.InteropServices.Marshalling.AnsiStringMarshaller))]
    private static partial IntPtr Dlsym(IntPtr handle, string symbol);

    [LibraryImport("libdl.dylib", EntryPoint = "dlclose", SetLastError = true)]
    private static partial int Dlclose(IntPtr handle);


    private static IntPtr _library_handle;

    static LibraryLoader() {
        if (_library_handle != IntPtr.Zero)
            throw new Exception("Library already loaded.");

        if (OperatingSystem.IsWindows()) {
            _library_handle = LoadLibrary(DllName);
            if (_library_handle == IntPtr.Zero)
                throw new Exception($"Failed to load library {DllName}. Error: {Marshal.GetLastWin32Error()}");
        } else {
            const int RTLD_LAZY = 1;
            _library_handle = Dlopen(DllName, RTLD_LAZY);
            if (_library_handle == IntPtr.Zero)
                throw new Exception($"Failed to load library {DllName}.");
        }


        create_context = FindFunction<CreateContextDelegate>("create_context");
        destroy_context = FindFunction<DestroyContextDelegate>("destroy_context");
        get_graph = FindFunction<GetGraphDelegate>("get_graph");
        get_func_lib = FindFunction<GetFuncLibDelegate>("get_func_lib");
        register_callback = FindFunction<RegisterCallbackDelegate>("register_callback");
        destroy_ffi_buf = FindFunction<DestroyFfiBufDelegate>("destroy_ffi_buf");
        uuid_from_string_extern = FindFunction<uuid_from_string_externDelegate>("uuid_from_string_extern");
        uuid_new_v4_extern = FindFunction<uuid_new_v4_externDelegate>("uuid_new_v4_extern");
        uuid_to_string_extern = FindFunction<uuid_to_string_externDelegate>("uuid_to_string_extern");
    }


    static void Unload() {
        if (_library_handle == IntPtr.Zero)
            throw new Exception("Library not loaded.");

        if (OperatingSystem.IsWindows())
            FreeLibrary(_library_handle);
        else
            Dlclose(_library_handle);
    }

    private static T FindFunction<T>(string function_name)
        where T : Delegate {
        IntPtr function_ptr = OperatingSystem.IsWindows()
            ? GetProcAddress(_library_handle, function_name)
            : Dlsym(_library_handle, function_name);

        if (function_ptr == IntPtr.Zero)
            throw new Exception($"Failed to find function {function_name}.");

        return Marshal.GetDelegateForFunctionPointer<T>(function_ptr);
    }


    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate IntPtr CreateContextDelegate();

    internal static readonly CreateContextDelegate create_context;


    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate void DestroyContextDelegate(IntPtr ctx);

    internal static readonly DestroyContextDelegate destroy_context;


    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate FfiBufIntern GetGraphDelegate(IntPtr ctx);

    private static readonly GetGraphDelegate get_graph;

    internal static FfiBuf GetGraph(IntPtr ctx) {
        return get_graph(ctx);
    }


    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate FfiBufIntern GetFuncLibDelegate(IntPtr ctx);

    private static readonly GetFuncLibDelegate get_func_lib;

    internal static FfiBuf GetFuncLib(IntPtr ctx) {
        return get_func_lib(ctx);
    }


    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate void CallbackDelegate(ScenariumCore.CallbackType value);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate void RegisterCallbackDelegate(IntPtr ctx, CallbackDelegate callback);

    internal static readonly RegisterCallbackDelegate register_callback;


    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate void DestroyFfiBufDelegate(FfiBufIntern ffi_buf);

    internal static readonly DestroyFfiBufDelegate destroy_ffi_buf;


    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate Uuid uuid_new_v4_externDelegate();

    internal static readonly uuid_new_v4_externDelegate uuid_new_v4_extern;


    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate FfiBufIntern uuid_to_string_externDelegate(Uuid uuid);

    private static readonly uuid_to_string_externDelegate uuid_to_string_extern;
    
    internal static FfiBuf uuidToString(Uuid uuid) {
        return uuid_to_string_extern(uuid);
    }


    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate Uuid uuid_from_string_externDelegate(FfiBufIntern buf);

    internal static readonly uuid_from_string_externDelegate uuid_from_string_extern;
}