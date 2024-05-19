using System;
using System.Collections.Generic;
using System.Collections.Specialized;
using System.Diagnostics;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Documents;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Navigation;
using System.Windows.Shapes;
using ScenariumEditor.NET.ViewModel;

namespace ScenariumEditor.NET;

public partial class MainWindow : Window {
    private readonly MainWindowViewModel _viewModel = new DesignMainWindowViewModel();

    public MainWindow() {
        InitializeComponent();

        this.DataContext = _viewModel;
    }


    private void AddDesignNodeButton_OnClick(object sender, RoutedEventArgs e) {
        _viewModel.Nodes.Add(new DesignNode());
    }

    private void MainWindow_OnLoaded(object sender, RoutedEventArgs e) {
    }


    private void CanvasgBg_OnLoaded(object sender, RoutedEventArgs e) {
        var uiElement = (UIElement)sender;
        uiElement.MouseMove += CanvasgBg_OnMouseMove_CanvasDragging;
        uiElement.MouseUp += CanvasgBg_OnButtonUp;

        uiElement.MouseMove += CanvasgBg_OnMouseMove_NewConnection;
        uiElement.PreviewMouseRightButtonDown += CanvasgBg_OnPreviewMouseRightButtonDown_NewConnection;
        uiElement.PreviewMouseLeftButtonDown += CanvasgBg_OnPreviewMouseLeftButtonDown_NewConnection;
    }

    #region canvas dragging

    private bool _isDragging = false;
    private Vector _canvasDragMousePosition;

    private void CanvasgBg_OnButtonDown(object sender, MouseButtonEventArgs e) {
        if (e.MiddleButton != MouseButtonState.Pressed) {
            return;
        }

        var canvas = (FrameworkElement)sender;
        if (canvas.CaptureMouse()) {
            _isDragging = true;
            _canvasDragMousePosition = e.GetPosition(canvas) - this._viewModel.CanvasPosition;
        }
    }

    private void CanvasgBg_OnButtonUp(object sender, MouseButtonEventArgs e) {
        var canvas = (FrameworkElement)sender;
        canvas.ReleaseMouseCapture();
        _isDragging = false;
    }

    private void CanvasgBg_OnMouseMove_CanvasDragging(object sender, MouseEventArgs e) {
        var canvas = (FrameworkElement)sender;
        var mousePosition = e.GetPosition(canvas);

        if (_isDragging) {
            var delta = mousePosition - _canvasDragMousePosition;
            this._viewModel.CanvasPosition = delta;
        }
    }

    #endregion

    private void CanvasgBg_OnMouseWheel(object sender, MouseWheelEventArgs e) {
        var canvas = (FrameworkElement)sender;
        var delta = e.Delta;
        this._viewModel.CanvasScale += delta / 2000.0;
    }

    #region new connection

    const double PinConnectionDistance = 80;

    private Point _canvasMousePosition;
    private bool _firstPinSelected = false;
    private Pin _activePin = null;
    private Pin _tempMousePin = null;
    private Pin _nearestPin = null;

    private void Node_OnPinActivated(object sender, Pin e) {
        if (!_firstPinSelected) {
            _firstPinSelected = true;

            _activePin = e;
            _tempMousePin = new Pin {
                DataType = e.DataType,
                PinType = e.PinType.GetOpposite(),
                CanvasPosition = _canvasMousePosition
            };
            NewConnectionCanvas.Visibility = Visibility.Visible;

            NewConnectionControl.DataContext = new Connection(_activePin, _tempMousePin);
            // NewConnectionControl.CaptureMouse();
        } else {
            _viewModel.Connections.Add(new Connection(_activePin, e));
            CancelNewConnection();
        }
    }

    private void CanvasgBg_OnMouseMove_NewConnection(object sender, MouseEventArgs e) {
        var canvas = (FrameworkElement)sender;
        _canvasMousePosition =
            e.GetPosition(canvas) - new Vector(_viewModel.CanvasPosition.X, _viewModel.CanvasPosition.Y);

        if (_tempMousePin != null) {
            _tempMousePin.CanvasPosition = _canvasMousePosition;
            _nearestPin = _viewModel.Nodes
                .SelectMany(
                    node => node.Events
                        .Concat(node.Inputs)
                        .Concat(node.Outputs)
                        .Concat([node.Trigger])
                )
                .Where(pin => pin.PinType == _tempMousePin.PinType && pin.DataType == _tempMousePin.DataType)
                .Where(pin => (pin.CanvasPosition - _tempMousePin.CanvasPosition).LengthSquared < PinConnectionDistance)
                .MinBy(pin => (pin.CanvasPosition - _tempMousePin.CanvasPosition).LengthSquared);
            if (_nearestPin != null) {
                _tempMousePin.CanvasPosition = _nearestPin.CanvasPosition;
            }
        }
    }

    private void CancelNewConnection() {
        _firstPinSelected = false;
        _tempMousePin = null;
        NewConnectionCanvas.Visibility = Visibility.Collapsed;
        NewConnectionControl.DataContext = null;
    }

    private void CanvasgBg_OnPreviewMouseLeftButtonDown_NewConnection(object sender, MouseButtonEventArgs e) {
        if (_firstPinSelected && _nearestPin != null) {
            _viewModel.Connections.Add(new Connection(_activePin, _nearestPin));
            CancelNewConnection();
            e.Handled = true;
        }
    }

    private void CanvasgBg_OnPreviewMouseRightButtonDown_NewConnection(object sender, MouseButtonEventArgs e) {
        if (_firstPinSelected) {
            CancelNewConnection();
            e.Handled = true;
        }
    }

    #endregion
}