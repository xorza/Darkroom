namespace csso.Common;

public static class Xtensions {
    public static IEnumerable<T> SkipNulls<T>(this IEnumerable<T?> source) {
        return source.Where(item => item != null)!
            .Select(_ => _!);
    }
}