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
        nameof(InputPosition),
        typeof(Point),
        typeof(ConnectionControl),
        new PropertyMetadata(Position_PropertyChangedCallback)
    );

    public static readonly DependencyProperty OutputPositionDependencyProperty = DependencyProperty.Register(
        nameof(OutputPosition),
        typeof(Point),
        typeof(ConnectionControl),
        new PropertyMetadata(Position_PropertyChangedCallback)
    );

    public static readonly DependencyProperty HoverBrushDependencyProperty = DependencyProperty.Register(
        nameof(HoverBrush),
        typeof(Brush),
        typeof(ConnectionControl),
        new PropertyMetadata(Brushes.Coral)
    );
    
    public ConnectionControl() {
        LeftButtonClick += LeftButtonClickHandler;
        MouseDoubleClick += MouseButtonEventHandler;
    }

    public static readonly DependencyProperty ThicknessProperty = DependencyProperty.Register(
        nameof(Thickness), typeof(double), typeof(ConnectionControl), new PropertyMetadata(default(double)));

    public double Thickness {
        get { return (double)GetValue(ThicknessProperty); }
        set { SetValue(ThicknessProperty, value); }
    }

    public Point InputPosition {
        get => (Point)GetValue(InputPositionDependencyProperty);
        set => SetValue(InputPositionDependencyProperty, value);
    }

    public Point OutputPosition {
        get => (Point)GetValue(OutputPositionDependencyProperty);
        set => SetValue(OutputPositionDependencyProperty, value);
    }

    public Brush HoverBrush {
        get => (Brush)GetValue(HoverBrushDependencyProperty);
        set => SetValue(HoverBrushDependencyProperty, value);
    }

    private static void Position_PropertyChangedCallback(DependencyObject d, DependencyPropertyChangedEventArgs e) {
        ((UIElement)d).InvalidateVisual();
    }

    private void LeftButtonClickHandler(object sender, MouseButtonEventArgs ea) {
    }

    private void MouseButtonEventHandler(object sender, MouseButtonEventArgs e) {
    }

    protected override void OnRender(DrawingContext drawingContext) {
        base.OnRender(drawingContext);

    
        Point[] points = [
            new(InputPosition.X - 5, InputPosition.Y),
            new(InputPosition.X - 50, InputPosition.Y),
            new(OutputPosition.X + 50, OutputPosition.Y),
            new(OutputPosition.X + 5, OutputPosition.Y)
        ];

        var pathFigure = new PathFigure {
            StartPoint = points[0]
        };
        pathFigure.Segments.Add(
            new BezierSegment(points[1], points[2], points[3], true));
        pathFigure.IsClosed = false;

        PathGeometry path = new();
        path.Figures.Add(pathFigure);

        drawingContext.DrawGeometry(null, new Pen(Brushes.Transparent, 5), path);
        drawingContext.DrawGeometry(null, new Pen(BorderBrush, Thickness), path);
        if (IsMouseOver) {
            drawingContext.DrawGeometry(null, new Pen(HoverBrush, Thickness * 1.5), path);
        }
    }
}