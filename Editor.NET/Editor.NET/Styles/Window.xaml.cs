using System.Diagnostics;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Shapes;

namespace Editor.NET.Styles;

class NormalWindowPosition {
    public Point Position { get; set; }
    public Size Size { get; set; }
}

partial class WindowResourceDictionary : ResourceDictionary {
    #region min max close

    private void btnClose_Click(object sender, RoutedEventArgs e) {
        var wnd = Window.GetWindow((FrameworkElement)sender)!;
        wnd.Close();
    }

    private void btnMax_Click(object sender, RoutedEventArgs e) {
        var wnd = Window.GetWindow((FrameworkElement)sender)!;
        if (wnd.WindowState == WindowState.Maximized) {
            wnd.WindowState = WindowState.Normal;
            if (wnd.Tag is NormalWindowPosition normalPosition) {
                wnd.Left = normalPosition.Position.X;
                wnd.Top = normalPosition.Position.Y;
                wnd.Width = normalPosition.Size.Width;
                wnd.Height = normalPosition.Size.Height;
            }
        } else {
            wnd.Tag = new NormalWindowPosition {
                Position = new Point(wnd.Left, wnd.Top),
                Size = new Size(wnd.Width, wnd.Height)
            };
            wnd.WindowState = WindowState.Maximized;
        }
        e.Handled = true;
    }

    private void btnMin_Click(object sender, RoutedEventArgs e) {
        var wnd = Window.GetWindow((FrameworkElement)sender)!;
        wnd.WindowState = WindowState.Minimized;
    }

    #endregion

    #region resizing

    bool _resizeInProcess = false;

    private void borderRect_MouseLeftButtonDown(object sender, MouseButtonEventArgs e) {
        var senderRect = (Rectangle)sender;
        _resizeInProcess = true;
        senderRect.CaptureMouse();
    }

    private void borderRect_MouseLeftButtonUp(object sender, MouseButtonEventArgs e) {
        var senderRect = (Rectangle)sender;
        _resizeInProcess = false;
        senderRect.ReleaseMouseCapture();
    }

    private void borderRect_MouseMove(object sender, MouseEventArgs e) {
        if (_resizeInProcess) {
            var senderRect = (Rectangle)sender;
            var mainWindow = Window.GetWindow(senderRect)!;

            double width = e.GetPosition(mainWindow).X;
            double height = e.GetPosition(mainWindow).Y;
            senderRect.CaptureMouse();
            
            if (senderRect.Name.ToLower().Contains("right")) {
                if (width > 0)
                    mainWindow.Width = width;
            }

            if (senderRect.Name.ToLower().Contains("left")) {
                mainWindow.Left += width;
                width = mainWindow.Width - width;
                if (width > 0) {
                    mainWindow.Width = width;
                }
            }

            if (senderRect.Name.ToLower().Contains("bottom")) {
                if (height > 0) {
                    mainWindow.Height = height;
                }
            }

            if (senderRect.Name.ToLower().Contains("top")) {
                mainWindow.Top += height;
                height = mainWindow.Height - height;
                if (height > 0) {
                    mainWindow.Height = height;
                }
            }
        }
    }

    #endregion

    #region dragging
    
    private void Header_MouseLeftButtonDown(object sender, MouseButtonEventArgs e) {
        var header = (FrameworkElement)sender;
        var wnd = Window.GetWindow(header)!;
         wnd.DragMove();
         e.Handled = true;
    }

    #endregion
}