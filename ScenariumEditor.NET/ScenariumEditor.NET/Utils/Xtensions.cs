using System.Windows;

namespace ScenariumEditor.NET.Utils;

public static class Xtensions {
    public static Vector ToVector(this Point point) {
        return new Vector(point.X, point.Y);
    }
}