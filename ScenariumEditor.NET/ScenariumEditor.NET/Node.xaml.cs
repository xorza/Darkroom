using System;
using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using ScenariumEditor.NET.ViewModel;

namespace ScenariumEditor.NET;

public partial class Node : UserControl {
    public Node() {
        InitializeComponent();

        this.MouseDown += (sender, args) => { args.Handled = true; };
    }

    private void Node_OnLoaded(object sender, RoutedEventArgs e) {
        NodeDataContext.UpdatePinPositions();
    }

    // event for activating a Pin
    public event EventHandler<Pin> PinActivated = null;

    public static readonly DependencyProperty NodeDataContextProperty = DependencyProperty.Register(
        nameof(NodeDataContext),
        typeof(ViewModel.Node),
        typeof(Node),
        new PropertyMetadata(default(ViewModel.Node), NodeDataContextPropertyChangedCallback)
    );

    private static void
        NodeDataContextPropertyChangedCallback(DependencyObject d, DependencyPropertyChangedEventArgs e) {
        ((Node)d).DataContext = e.NewValue;
    }

    public ViewModel.Node NodeDataContext {
        get { return (ViewModel.Node)GetValue(NodeDataContextProperty); }
        set { SetValue(NodeDataContextProperty, value); }
    }

    #region dragging

    private bool _isDragging = false;
    private Point _headerDragMousePosition;

    private void Header_OnMouseLeftButtonDown(object sender, MouseButtonEventArgs e) {
        var header = (FrameworkElement)sender!;
        if (header.CaptureMouse()) {
            _headerDragMousePosition = e.GetPosition(header);
            _isDragging = true;
            e.Handled = true;
        }
    }

    private void Header_OnMouseLeftButtonUp(object sender, MouseButtonEventArgs e) {
        var header = (FrameworkElement)sender!;
        _isDragging = false;
        header.ReleaseMouseCapture();
    }

    private void Header_OnMouseMove(object sender, MouseEventArgs e) {
        if (!_isDragging) return;

        var header = (FrameworkElement)sender!;
        var currentPosition = e.GetPosition(header);
        var delta = currentPosition - _headerDragMousePosition;

        NodeDataContext.CanvasPosition += delta;
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
        PinActivated?.Invoke(this, pin);
    }
}