using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;

namespace ScenariumEditor.NET;

public class ClickControl : Control {
    private bool _leftButtonDown;

    protected ClickControl() {
        MouseEnter += MouseEnterHandler;
        MouseLeave += MouseEnterHandler;
        MouseLeftButtonDown += MouseLeftButtonDownHandler;
        MouseLeftButtonUp += MouseLeftButtonUpHandler;
    }

    public event MouseButtonEventHandler LeftButtonClick;

    private void MouseEnterHandler(object sender, RoutedEventArgs ea) {
        _leftButtonDown = false;

        InvalidateVisual();
    }

    private void MouseLeftButtonDownHandler(object sender, MouseButtonEventArgs ea) {
        _leftButtonDown = true;

        InvalidateVisual();
    }

    private void MouseLeftButtonUpHandler(object sender, MouseButtonEventArgs ea) {
        if (_leftButtonDown) {
            LeftButtonClick?.Invoke(sender, ea);
        }

        _leftButtonDown = false;

        InvalidateVisual();
    }
}