using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace GraphLib.Controls;

public class ClippingBorder : Border {
    private readonly RectangleGeometry _clip_rect = new RectangleGeometry();
    
    protected override void OnRender(DrawingContext dc) {
        _clip_rect.RadiusX = _clip_rect.RadiusY =
            Math.Max(0.0, this.CornerRadius.TopLeft - (this.BorderThickness.Left * 0.5));
        _clip_rect.Rect = new Rect(this.RenderSize);
        this.Clip = _clip_rect;
        
        base.OnRender(dc);
    }
}
