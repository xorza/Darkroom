using System.Windows;

namespace GraphLib.Utils;

public static class Xtensions {
    public static Vector ToVector(this Point point) {
        return new Vector(point.X, point.Y);
    }
}