using System.Windows;

namespace GraphLib.Utils;

public class BindingProxy : Freezable {
    public static readonly DependencyProperty DATA_CONTEXT_PROPERTY = DependencyProperty.Register (
        nameof(DataContext),
        typeof (object),
        typeof (BindingProxy));

    public object DataContext {
        get => GetValue (DATA_CONTEXT_PROPERTY);
        set => SetValue (DATA_CONTEXT_PROPERTY, value);
    }

    protected override Freezable CreateInstanceCore () {
        return new BindingProxy ();
    }
}