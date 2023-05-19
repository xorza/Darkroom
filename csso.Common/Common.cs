namespace csso.Common;

public static class MyDebug {
#if DEBUG
    public const bool IsDebug = true;
#else
    public const bool IsDebug = false;
#endif
}