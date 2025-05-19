using System;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;

namespace GraphLib.Controls;

public class ConnectionControl : ClickControl {
    public static readonly DependencyProperty INPUT_POSITION_DEPENDENCY_PROPERTY = DependencyProperty.Register(
        nameof(Pin1Position),
        typeof(Point),
        typeof(ConnectionControl),
        new PropertyMetadata(OnPropertyChangedCallback_InvalidateVisual)
    );

    public static readonly DependencyProperty OUTPUT_POSITION_DEPENDENCY_PROPERTY = DependencyProperty.Register(
        nameof(Pin2Position),
        typeof(Point),
        typeof(ConnectionControl),
        new PropertyMetadata(OnPropertyChangedCallback_InvalidateVisual)
    );

    public static readonly DependencyProperty HOVER_BRUSH_DEPENDENCY_PROPERTY = DependencyProperty.Register(
        nameof(HoverBrush),
        typeof(Brush),
        typeof(ConnectionControl),
        new PropertyMetadata(Brushes.Coral, OnPropertyChangedCallback_InvalidateVisual)
    );

    public ConnectionControl() {
        LeftButtonClick += LeftButtonClickHandler;
        MouseDoubleClick += MouseButtonEventHandler;
    }

    public static readonly DependencyProperty THICKNESS_PROPERTY = DependencyProperty.Register(
        nameof(Thickness), typeof(double), typeof(ConnectionControl),
        new PropertyMetadata(2.0, OnPropertyChangedCallback_InvalidateVisual)
    );

    public double Thickness {
        get { return (double)GetValue(THICKNESS_PROPERTY); }
        set { SetValue(THICKNESS_PROPERTY, value); }
    }

    public static readonly DependencyProperty HOVER_THICKNESS_PROPERTY = DependencyProperty.Register(
        nameof(HoverThickness), typeof(double), typeof(ConnectionControl),
        new PropertyMetadata(3.0, OnPropertyChangedCallback_InvalidateVisual)
    );

    public static readonly DependencyProperty DELETED_BRUSH_PROPERTY = DependencyProperty.Register(
        nameof(DeletedBrush), typeof(Brush), typeof(ConnectionControl),
        new PropertyMetadata(Brushes.IndianRed, OnPropertyChangedCallback_InvalidateVisual)
    );

    public static readonly DependencyProperty IS_DELETED_PROPERTY = DependencyProperty.Register(
        nameof(IsDeleted), typeof(bool), typeof(ConnectionControl),
        new PropertyMetadata(default(bool), OnPropertyChangedCallback_InvalidateVisual)
    );

    public static readonly DependencyProperty EVENT_BRUSH_PROPERTY = DependencyProperty.Register(
        nameof(EventBrush), typeof(Brush), typeof(ConnectionControl),
        new PropertyMetadata(default(Brush), OnPropertyChangedCallback_InvalidateVisual)
    );

    public static readonly DependencyProperty IS_EVENT_PROPERTY = DependencyProperty.Register(
        nameof(IsEvent), typeof(bool), typeof(ConnectionControl),
        new PropertyMetadata(default(bool), OnPropertyChangedCallback_InvalidateVisual)
    );

    public bool IsEvent {
        get { return (bool)GetValue(IS_EVENT_PROPERTY); }
        set { SetValue(IS_EVENT_PROPERTY, value); }
    }

    public Brush EventBrush {
        get { return (Brush)GetValue(EVENT_BRUSH_PROPERTY); }
        set { SetValue(EVENT_BRUSH_PROPERTY, value); }
    }

    public double HoverThickness {
        get { return (double)GetValue(HOVER_THICKNESS_PROPERTY); }
        set { SetValue(HOVER_THICKNESS_PROPERTY, value); }
    }

    public Point Pin1Position {
        get => (Point)GetValue(INPUT_POSITION_DEPENDENCY_PROPERTY);
        set => SetValue(INPUT_POSITION_DEPENDENCY_PROPERTY, value);
    }

    public Point Pin2Position {
        get => (Point)GetValue(OUTPUT_POSITION_DEPENDENCY_PROPERTY);
        set => SetValue(OUTPUT_POSITION_DEPENDENCY_PROPERTY, value);
    }

    public Brush HoverBrush {
        get => (Brush)GetValue(HOVER_BRUSH_DEPENDENCY_PROPERTY);
        set => SetValue(HOVER_BRUSH_DEPENDENCY_PROPERTY, value);
    }

    public Brush DeletedBrush {
        get { return (Brush)GetValue(DELETED_BRUSH_PROPERTY); }
        set { SetValue(DELETED_BRUSH_PROPERTY, value); }
    }

    public bool IsDeleted {
        get { return (bool)GetValue(IS_DELETED_PROPERTY); }
        set { SetValue(IS_DELETED_PROPERTY, value); }
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

    protected override void OnRender(DrawingContext drawing_context) {
        base.OnRender(drawing_context);

        Point[] points = [
            new(Pin1Position.X - 5, Pin1Position.Y),
            new(Pin1Position.X - 50, Pin1Position.Y),
            new(Pin2Position.X + 50, Pin2Position.Y),
            new(Pin2Position.X + 5, Pin2Position.Y)
        ];

        var path_figure = new PathFigure(
            points[0],
            [new BezierSegment(points[1], points[2], points[3], true)],
            false
        );

        Geometry.Clear();
        Geometry.Figures.Add(path_figure);

        // transparent line to make the connection easier to click
        drawing_context.DrawGeometry(null, new Pen(Brushes.Transparent, 5), Geometry);


        if (IsDeleted) {
            drawing_context.DrawGeometry(null, new Pen(DeletedBrush, Thickness), Geometry);
        } else if (IsMouseOver) {
            drawing_context.DrawGeometry(null, new Pen(HoverBrush, HoverThickness), Geometry);
        } else if (IsEvent) {
            drawing_context.DrawGeometry(null, new Pen(EventBrush, Thickness), Geometry);
        } else {
            drawing_context.DrawGeometry(null, new Pen(BorderBrush, Thickness), Geometry);
        }
    }
}