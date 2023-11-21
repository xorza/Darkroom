using System;
using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using Editor.NET.ViewModel;

namespace Editor.NET;

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

    private void Title_OnMouseLeftButtonDown(object sender, MouseButtonEventArgs e) {
        this.MouseMove += Title_OnMouseMove;
        _dragTitleMousePosition = e.GetPosition(this);
    }

    private void Title_OnMouseLeftButtonUp(object sender, MouseButtonEventArgs e) {
        this.MouseMove -= Title_OnMouseMove;
    }

    private void Title_OnMouseMove(object sender, MouseEventArgs e) {
        var currentPosition = e.GetPosition(this);
        var delta = currentPosition - _dragTitleMousePosition;

        NodeDataContext.CanvasPosition = new Point(
            NodeDataContext.CanvasPosition.X + delta.X,
            NodeDataContext.CanvasPosition.Y + delta.Y
        );
    }

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