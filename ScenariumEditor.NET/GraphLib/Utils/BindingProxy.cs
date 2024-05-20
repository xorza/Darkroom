using System.Windows;

namespace GraphLib.Utils;

public class BindingProxy : Freezable {
    public static readonly DependencyProperty DataContextProperty = DependencyProperty.Register (
        nameof(DataContext),
        typeof (object),
        typeof (BindingProxy));

    public object DataContext {
        get => GetValue (DataContextProperty);
        set => SetValue (DataContextProperty, value);
    }

    protected override Freezable CreateInstanceCore () {
        return new BindingProxy ();
    }
}