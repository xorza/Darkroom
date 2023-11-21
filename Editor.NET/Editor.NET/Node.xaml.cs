using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;

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

        NodeDataContext.Position = new Point(
            NodeDataContext.Position.X + delta.X,
            NodeDataContext.Position.Y + delta.Y
        );
    }
}