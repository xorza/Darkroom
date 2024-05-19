using System;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;

namespace ScenariumEditor.NET;

public class ConnectionEventArgs : RoutedEventArgs {
    public ConnectionEventArgs(ConnectionControl connectionControl) {
        ConnectionControl = connectionControl;
    }

    public ConnectionControl ConnectionControl { get; }
}

public class ConnectionControl : ClickControl {
    public static readonly DependencyProperty InputPositionDependencyProperty = DependencyProperty.Register(
        nameof(Pin1Position),
        typeof(Point),
        typeof(ConnectionControl),
        new PropertyMetadata(OnPropertyChangedCallback_InvalidateVisual)
    );

    public static readonly DependencyProperty OutputPositionDependencyProperty = DependencyProperty.Register(
        nameof(Pin2Position),
        typeof(Point),
        typeof(ConnectionControl),
        new PropertyMetadata(OnPropertyChangedCallback_InvalidateVisual)
    );

    public static readonly DependencyProperty HoverBrushDependencyProperty = DependencyProperty.Register(
        nameof(HoverBrush),
        typeof(Brush),
        typeof(ConnectionControl),
        new PropertyMetadata(Brushes.Coral, OnPropertyChangedCallback_InvalidateVisual)
    );

    public ConnectionControl() {
        LeftButtonClick += LeftButtonClickHandler;
        MouseDoubleClick += MouseButtonEventHandler;
    }

    public static readonly DependencyProperty ThicknessProperty = DependencyProperty.Register(
        nameof(Thickness), typeof(double), typeof(ConnectionControl),
        new PropertyMetadata(2.0, OnPropertyChangedCallback_InvalidateVisual)
    );

    public double Thickness {
        get { return (double)GetValue(ThicknessProperty); }
        set { SetValue(ThicknessProperty, value); }
    }

    public static readonly DependencyProperty HoverThicknessProperty = DependencyProperty.Register(
        nameof(HoverThickness), typeof(double), typeof(ConnectionControl),
        new PropertyMetadata(3.0, OnPropertyChangedCallback_InvalidateVisual)
    );

    public double HoverThickness {
        get { return (double)GetValue(HoverThicknessProperty); }
        set { SetValue(HoverThicknessProperty, value); }
    }

    public Point Pin1Position {
        get => (Point)GetValue(InputPositionDependencyProperty);
        set => SetValue(InputPositionDependencyProperty, value);
    }

    public Point Pin2Position {
        get => (Point)GetValue(OutputPositionDependencyProperty);
        set => SetValue(OutputPositionDependencyProperty, value);
    }

    public Brush HoverBrush {
        get => (Brush)GetValue(HoverBrushDependencyProperty);
        set => SetValue(HoverBrushDependencyProperty, value);
    }

    public static readonly DependencyProperty DeletedBrushProperty = DependencyProperty.Register(
        nameof(DeletedBrush), typeof(Brush), typeof(ConnectionControl),
        new PropertyMetadata(Brushes.IndianRed, OnPropertyChangedCallback_InvalidateVisual)
    );

    public Brush DeletedBrush {
        get { return (Brush)GetValue(DeletedBrushProperty); }
        set { SetValue(DeletedBrushProperty, value); }
    }

    public static readonly DependencyProperty IsDeletedProperty = DependencyProperty.Register(
        nameof(IsDeleted), typeof(bool), typeof(ConnectionControl),
        new PropertyMetadata(default(bool), OnPropertyChangedCallback_InvalidateVisual)
    );

    public bool IsDeleted {
        get { return (bool)GetValue(IsDeletedProperty); }
        set { SetValue(IsDeletedProperty, value); }
    }

    public PathGeometry Geometry { get; } = new();

    private static void OnPropertyChangedCallback_InvalidateVisual(DependencyObject d,
        DependencyPropertyChangedEventArgs e) {
        ((UIElement)d).InvalidateVisual();
    }

    private void LeftButtonClickHandler(object sender, MouseButtonEventArgs ea) {
    }

    private void MouseButtonEventHandler(object sender, MouseButtonEventArgs e) {
    }

    protected override void OnRender(DrawingContext drawingContext) {
        base.OnRender(drawingContext);

        Point[] points = [
            new(Pin1Position.X - 5, Pin1Position.Y),
            new(Pin1Position.X - 50, Pin1Position.Y),
            new(Pin2Position.X + 50, Pin2Position.Y),
            new(Pin2Position.X + 5, Pin2Position.Y)
        ];

        var pathFigure = new PathFigure(
            points[0],
            [new BezierSegment(points[1], points[2], points[3], true)],
            false
        );

        Geometry.Clear();
        Geometry.Figures.Add(pathFigure);

        // transparent line to make the connection easier to click
        drawingContext.DrawGeometry(null, new Pen(Brushes.Transparent, 5), Geometry);


        if (IsDeleted) {
            drawingContext.DrawGeometry(null, new Pen(DeletedBrush, Thickness), Geometry);
        } else if (IsMouseOver) {
            drawingContext.DrawGeometry(null, new Pen(HoverBrush, HoverThickness), Geometry);
        } else {
            drawingContext.DrawGeometry(null, new Pen(BorderBrush, Thickness), Geometry);
        }
    }
}