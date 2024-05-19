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
            _canvasDragMousePosition = e.GetPosition(canvas) -  this._viewModel.CanvasPosition;
        }
    }

    private void CanvasgBg_OnButtonUp(object sender, MouseButtonEventArgs e) {
        var canvas = (FrameworkElement)sender;
        canvas.ReleaseMouseCapture();
        _isDragging = false;
    }

    private void CanvasgBg_OnMouseMove(object sender, MouseEventArgs e) {
        if (!_isDragging) return;
        var canvas = (FrameworkElement)sender;
        var currentPosition = e.GetPosition(canvas);
        var delta = currentPosition - _canvasDragMousePosition;
        this._viewModel.CanvasPosition = delta;
    }

    #endregion

    private void CanvasgBg_OnMouseWheel(object sender, MouseWheelEventArgs e) {
        var canvas = (FrameworkElement)sender;
        var delta = e.Delta;
        this._viewModel.CanvasScale += delta / 2000.0;
    }
}