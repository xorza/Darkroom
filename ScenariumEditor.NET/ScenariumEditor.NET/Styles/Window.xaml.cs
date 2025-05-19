using System.Diagnostics;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Shapes;

namespace ScenariumEditor.NET.Styles;

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
            if (wnd.Tag is NormalWindowPosition normal_position) {
                wnd.Left = normal_position.Position.X;
                wnd.Top = normal_position.Position.Y;
                wnd.Width = normal_position.Size.Width;
                wnd.Height = normal_position.Size.Height;
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

    bool _resize_in_process = false;

    private void borderRect_MouseLeftButtonDown(object sender, MouseButtonEventArgs e) {
        var sender_rect = (Rectangle)sender;
        _resize_in_process = true;
        sender_rect.CaptureMouse();
    }

    private void borderRect_MouseLeftButtonUp(object sender, MouseButtonEventArgs e) {
        var sender_rect = (Rectangle)sender;
        _resize_in_process = false;
        sender_rect.ReleaseMouseCapture();
    }

    private void borderRect_MouseMove(object sender, MouseEventArgs e) {
        if (_resize_in_process) {
            var sender_rect = (Rectangle)sender;
            var main_window = Window.GetWindow(sender_rect)!;

            double width = e.GetPosition(main_window).X;
            double height = e.GetPosition(main_window).Y;
            sender_rect.CaptureMouse();

            if (sender_rect.Name.ToLower().Contains("right")) {
                if (width > 0)
                    main_window.Width = width;
            }

            if (sender_rect.Name.ToLower().Contains("left")) {
                main_window.Left += width;
                width = main_window.Width - width;
                if (width > 0) {
                    main_window.Width = width;
                }
            }

            if (sender_rect.Name.ToLower().Contains("bottom")) {
                if (height > 0) {
                    main_window.Height = height;
                }
            }

            if (sender_rect.Name.ToLower().Contains("top")) {
                main_window.Top += height;
                height = main_window.Height - height;
                if (height > 0) {
                    main_window.Height = height;
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

    private void Window_Loaded(object sender, RoutedEventArgs e) {
        var self = (Window)sender;
        self.MaxHeight = SystemParameters.MaximizedPrimaryScreenHeight;
    }
}