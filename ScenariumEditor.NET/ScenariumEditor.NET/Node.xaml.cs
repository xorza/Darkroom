using System;
using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using ScenariumEditor.NET.ViewModel;

namespace ScenariumEditor.NET;

public partial class Node : UserControl {
    private Point _dragTitleMousePosition;

    public Node() {
        InitializeComponent();
    }

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
    private bool _isDragging=false;
    
    private void Title_OnMouseLeftButtonDown(object sender, MouseButtonEventArgs e) {
        var header = (FrameworkElement)sender!;
        if (header.CaptureMouse()) {
            _dragTitleMousePosition = e.GetPosition(header);
            _isDragging = true;
        }
    }

    private void Title_OnMouseLeftButtonUp(object sender, MouseButtonEventArgs e) {
        var header = (FrameworkElement)sender!;
        _isDragging = false;
        header.ReleaseMouseCapture();
    }

    private void Title_OnMouseMove(object sender, MouseEventArgs e) {
        if( !_isDragging ) return;
        
        var header = (FrameworkElement)sender!;
        var currentPosition = e.GetPosition(header);
        var delta = currentPosition - _dragTitleMousePosition;

        NodeDataContext.CanvasPosition = new Point(
            NodeDataContext.CanvasPosition.X + delta.X,
            NodeDataContext.CanvasPosition.Y + delta.Y
        );
    }
    #endregion

    private void Input_OnLoaded(object sender, RoutedEventArgs e) {
        var input = (FrameworkElement)sender!;
        var position = input.TranslatePoint(new Point(input.ActualWidth / 2.0f, input.ActualHeight / 2.0f), this);
        ((Input)input.DataContext).NodePosition = position;
    }

    private void Output_OnLoaded(object sender, RoutedEventArgs e) {
        var output = (FrameworkElement)sender!;
        var position = output.TranslatePoint(new Point(output.ActualWidth / 2.0f, output.ActualHeight / 2.0f), this);
        ((Output)output.DataContext).NodePosition = position;
    }

    private void Node_OnLoaded(object sender, RoutedEventArgs e) {
        NodeDataContext.UpdatePinPositions();
    }
}