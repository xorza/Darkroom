using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Shapes;

namespace Editor.NET.Styles;

partial class WindowResourceDictionary : ResourceDictionary {
    private void btnClose_Click(object sender, RoutedEventArgs e) {
        var wnd = (Window)((FrameworkElement)sender).Tag;
        wnd.Close();
    }

    private void btnMax_Click(object sender, RoutedEventArgs e) {
        var wnd = (Window)((FrameworkElement)sender).Tag;
        wnd.WindowState = wnd.WindowState == WindowState.Maximized ? WindowState.Normal : WindowState.Maximized;
    }

    private void btnMin_Click(object sender, RoutedEventArgs e) {
        var wnd = (Window)((FrameworkElement)sender).Tag;
        wnd.WindowState = WindowState.Minimized;
    }

    #region ResizeWindows

    bool resizeInProcess = false;

    private void borderRect_MouseLeftButtonDown(object sender, MouseButtonEventArgs e) {
        var senderRect = (Rectangle)sender;
        resizeInProcess = true;
        senderRect.CaptureMouse();
    }

    private void borderRect_MouseLeftButtonUp(object sender, MouseButtonEventArgs e) {
        var senderRect = (Rectangle)sender;
        resizeInProcess = false;
        senderRect.ReleaseMouseCapture();
    }

    private void borderRect_MouseMove(object sender, MouseEventArgs e) {
        if (resizeInProcess) {
            var senderRect = (Rectangle)sender;
            var mainWindow = (Window)senderRect.Tag;

            double width = e.GetPosition(mainWindow).X;
            double height = e.GetPosition(mainWindow).Y;
            senderRect.CaptureMouse();
            if (senderRect.Name.ToLower().Contains("right")) {
                width += 5;
                if (width > 0)
                    mainWindow.Width = width;
            }

            if (senderRect.Name.ToLower().Contains("left")) {
                width -= 5;
                mainWindow.Left += width;
                width = mainWindow.Width - width;
                if (width > 0) {
                    mainWindow.Width = width;
                }
            }

            if (senderRect.Name.ToLower().Contains("bottom")) {
                height += 5;
                if (height > 0)
                    mainWindow.Height = height;
            }

            if (senderRect.Name.ToLower().Contains("top")) {
                height -= 5;
                mainWindow.Top += height;
                height = mainWindow.Height - height;
                if (height > 0) {
                    mainWindow.Height = height;
                }
            }
        }
    }

    #endregion
}