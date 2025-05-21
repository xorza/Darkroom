using System.Runtime.InteropServices;

namespace CoreInterop.Utils;

internal unsafe partial struct FfiBufIntern : IDisposable {

    public byte* Bytes = null;
    public uint Length = 0;
    public uint Capacity = 0;

    public FfiBufIntern() {
    }

    public FfiBufIntern(byte* bytes, uint length, uint capacity) {
        Bytes = bytes;
        Length = length;
        Capacity = capacity;
    }

    public void Dispose() {
        if (Bytes != null) {
            LibraryLoader.destroy_ffi_buf(this);
        }

        Bytes = null;
    }
}

internal unsafe class FfiBuf : IDisposable {
    internal FfiBufIntern _buf_intern;
    private bool _disposed = false;

    public static implicit operator FfiBuf(FfiBufIntern value) {
        var newbuf = new FfiBuf();
        newbuf._buf_intern = value;
        return newbuf;
    }

    private FfiBuf() {
    }

    public FfiBuf(String s) {
        var bytes = Marshal.StringToHGlobalAnsi(s);

        _buf_intern = new FfiBufIntern((byte*)bytes, (uint)s.Length, (uint)s.Length);
    }

    public FfiBuf(byte[] array) {
        var bytes = Marshal.AllocHGlobal(array.Length);
        Marshal.Copy(array, 0, bytes, array.Length);

        _buf_intern = new FfiBufIntern((byte*)bytes, (uint)array.Length, (uint)array.Length);
    }

    public FfiBuf(List<byte> list) {
        var array = list.ToArray();
        var bytes = Marshal.AllocHGlobal(array.Length);
        Marshal.Copy(array, 0, bytes, array.Length);

        _buf_intern = new FfiBufIntern((byte*)bytes, (uint)array.Length, (uint)array.Length);
    }

    ~FfiBuf() {
        ReleaseUnmanagedResources();
    }

    private void ReleaseUnmanagedResources() {
        if (_disposed) return;
        _buf_intern.Dispose();
        _disposed = true;
    }

    public void Dispose() {
        ReleaseUnmanagedResources();
        GC.SuppressFinalize(this);
    }

    public override String ToString() {
        if (_buf_intern.Bytes == null) throw new InvalidOperationException("Disposed buffer");
        return Marshal.PtrToStringAnsi((IntPtr)_buf_intern.Bytes, (int)_buf_intern.Length);
    }

    public byte[] ToArray() {
        if (_buf_intern.Bytes == null) throw new InvalidOperationException("Disposed buffer");

        var result = new byte[_buf_intern.Length];
        Marshal.Copy((IntPtr)_buf_intern.Bytes, result, 0, (int)_buf_intern.Length);
        return result;
    }

    public List<byte> ToList() {
        return this.ToArray().ToList();
    }

    public T[] ToArray<T>() where T : unmanaged {
        if (_buf_intern.Bytes == null) throw new InvalidOperationException("Disposed buffer");

        var type = typeof(T);

        if (!type.IsPrimitive) {
            if (type.IsValueType) {
                if (!type.IsLayoutSequential && !type.IsExplicitLayout) {
                    throw new InvalidOperationException(string.Format("{0} does not define a StructLayout attribute",
                        type));
                }
            } else {
                throw new InvalidOperationException(string.Format("{0} is not a primitive or value type", type));
            }
        }

        var t_size = Marshal.SizeOf<T>();
        if (this._buf_intern.Length % t_size != 0) throw new InvalidOperationException("Invalid array size");

        var t_length = (int)this._buf_intern.Length / t_size;
        var result = new T[t_length];

        if (t_length == 0) return result;

        GCHandle handle = new GCHandle();
        try {
            // Make sure the array won't be moved around by the GC 
            handle = GCHandle.Alloc(result, GCHandleType.Pinned);
            var destination = handle.AddrOfPinnedObject().ToPointer();
            Buffer.MemoryCopy(_buf_intern.Bytes, destination, _buf_intern.Length, _buf_intern.Length);
        } finally {
            if (handle.IsAllocated)
                handle.Free();
        }

        return result;
    }

    public List<T> ToList<T>() where T : unmanaged {
        return this.ToArray<T>().ToList();
    }
}