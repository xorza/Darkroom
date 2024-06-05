using System;
using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using GraphLib.ViewModel;

namespace GraphLib.Controls;

public partial class NodeControl : UserControl {
    private Node _view_model = null!;

    public NodeControl() {
        InitializeComponent();
    }

    private void Node_OnLoaded(object sender, RoutedEventArgs e) {
        _view_model = (Node)DataContext;
        _view_model.UpdatePinPositions();
    }

    public event EventHandler<Pin> PinClick = null;
    public event EventHandler DeletePressed = null;
    public event EventHandler Selected = null;


    #region dragging

    private bool _is_dragging = false;
    private Point _header_drag_mouse_position;

    private void Header_OnMouseLeftButtonDown(object sender, MouseButtonEventArgs e) {
        var header = (FrameworkElement)sender!;
        Selected?.Invoke(this, EventArgs.Empty);
        if (header.CaptureMouse()) {
            _header_drag_mouse_position = e.GetPosition(header);
            _is_dragging = true;
            e.Handled = true;
        }
    }

    private void Header_OnMouseLeftButtonUp(object sender, MouseButtonEventArgs e) {
        var header = (FrameworkElement)sender!;
        _is_dragging = false;
        header.ReleaseMouseCapture();
    }

    private void Header_OnMouseMove(object sender, MouseEventArgs e) {
        if (!_is_dragging) return;

        var header = (FrameworkElement)sender!;
        var current_position = e.GetPosition(header);
        var delta = current_position - _header_drag_mouse_position;

        _view_model.CanvasPosition += delta;
    }

    #endregion

    private void PinButton_OnLoaded(object sender, RoutedEventArgs e) {
        var element = (FrameworkElement)sender!;
        var position = element.TranslatePoint(new Point(element.ActualWidth / 2.0f, element.ActualHeight / 2.0f), this);
        var pin = (Pin)element.DataContext!;
        pin.NodePosition = position;
    }

    private void PinButton_OnClick(object sender, RoutedEventArgs e) {
        var element = (FrameworkElement)sender;
        var pin = (Pin)element.DataContext!;
        PinClick?.Invoke(this, pin);
    }

    private void DeleteButton_OnClick(object sender, RoutedEventArgs e) {
        DeletePressed?.Invoke(this, EventArgs.Empty);
    }

    private void Node_OnMouseDown(object sender, MouseButtonEventArgs e) {
        Selected?.Invoke(this, EventArgs.Empty);
        e.Handled = true;
    }
}