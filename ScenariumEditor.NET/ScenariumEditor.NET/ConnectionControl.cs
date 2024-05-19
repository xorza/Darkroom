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
        new PropertyMetadata(Position_PropertyChangedCallback)
    );

    public static readonly DependencyProperty OutputPositionDependencyProperty = DependencyProperty.Register(
        nameof(Pin2Position),
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
        nameof(Thickness), typeof(double), typeof(ConnectionControl), new PropertyMetadata(2.0));

    public double Thickness {
        get { return (double)GetValue(ThicknessProperty); }
        set { SetValue(ThicknessProperty, value); }
    }
    
    public static readonly DependencyProperty HoverThicknessProperty = DependencyProperty.Register(
        nameof(HoverThickness), typeof(double), typeof(ConnectionControl), new PropertyMetadata(3.0));

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
            new(Pin1Position.X - 5, Pin1Position.Y),
            new(Pin1Position.X - 50, Pin1Position.Y),
            new(Pin2Position.X + 50, Pin2Position.Y),
            new(Pin2Position.X + 5, Pin2Position.Y)
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
            drawingContext.DrawGeometry(null, new Pen(HoverBrush, HoverThickness), path);
        }
    }
}