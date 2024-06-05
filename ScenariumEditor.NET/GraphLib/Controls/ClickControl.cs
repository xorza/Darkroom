using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;

namespace GraphLib.Controls;

public class ClickControl : Control {
    private bool _left_button_down;

    protected ClickControl() {
        MouseEnter += MouseEnterHandler;
        MouseLeave += MouseEnterHandler;
        MouseLeftButtonDown += MouseLeftButtonDownHandler;
        MouseLeftButtonUp += MouseLeftButtonUpHandler;
    }

    public event MouseButtonEventHandler LeftButtonClick;

    private void MouseEnterHandler(object sender, RoutedEventArgs ea) {
        _left_button_down = false;

        InvalidateVisual();
    }

    private void MouseLeftButtonDownHandler(object sender, MouseButtonEventArgs ea) {
        _left_button_down = true;

        InvalidateVisual();
    }

    private void MouseLeftButtonUpHandler(object sender, MouseButtonEventArgs ea) {
        if (_left_button_down) {
            LeftButtonClick?.Invoke(sender, ea);
        }

        _left_button_down = false;

        InvalidateVisual();
    }
}