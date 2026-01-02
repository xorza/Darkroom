use eframe::egui;
use graph::graph::NodeId;
use graph::prelude::FuncLib;
use std::collections::HashMap;
use std::marker::PhantomData;

use crate::{
    gui::{node, style::Style},
    model,
};

#[derive(Debug, Clone, Copy)]
pub struct UiRef<'a> {
    ptr: *const egui::Ui,
    _marker: PhantomData<&'a egui::Ui>,
}

impl<'a> UiRef<'a> {
    pub fn new(ui: &'a egui::Ui) -> Self {
        Self {
            ptr: ui as *const egui::Ui,
            _marker: PhantomData,
        }
    }

    pub fn get(&self) -> &'a egui::Ui {
        assert!(!self.ptr.is_null(), "ui pointer must not be null");
        unsafe { &*self.ptr }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PainterRef<'a> {
    ptr: *const egui::Painter,
    _marker: PhantomData<&'a egui::Painter>,
}

impl<'a> PainterRef<'a> {
    pub fn new(painter: &'a egui::Painter) -> Self {
        Self {
            ptr: painter as *const egui::Painter,
            _marker: PhantomData,
        }
    }

    pub fn get(&self) -> &'a egui::Painter {
        assert!(!self.ptr.is_null(), "painter pointer must not be null");
        unsafe { &*self.ptr }
    }
}

#[derive(Debug)]
pub struct RenderContext<'a> {
    ui: UiRef<'a>,
    painter: PainterRef<'a>,
    pub rect: egui::Rect,
    pub origin: egui::Pos2,
    pub layout: node::NodeLayout,
    pub style: Style,
    pub node_widths: HashMap<NodeId, f32>,
    pub scale: f32,
}

impl<'a> RenderContext<'a> {
    pub fn new(
        ui: &'a egui::Ui,
        painter: &'a egui::Painter,
        rect: egui::Rect,
        view_graph: &model::ViewGraph,
        func_lib: &FuncLib,
    ) -> Self {
        assert!(view_graph.zoom.is_finite(), "graph zoom must be finite");
        assert!(view_graph.zoom > 0.0, "graph zoom must be positive");
        assert!(view_graph.pan.x.is_finite(), "graph pan x must be finite");
        assert!(view_graph.pan.y.is_finite(), "graph pan y must be finite");

        let layout = node::NodeLayout::default().scaled(view_graph.zoom);

        let style = Style::new(ui, view_graph.zoom);

        let width_ctx = node::NodeWidthContext {
            layout: &layout,
            style: &style,
            scale: view_graph.zoom,
        };
        let node_widths = node::compute_node_widths(painter, view_graph, func_lib, &width_ctx);
        let origin = rect.min + view_graph.pan;

        Self {
            ui: UiRef::new(ui),
            painter: PainterRef::new(painter),
            rect,
            origin,
            layout,
            style,
            node_widths,
            scale: view_graph.zoom,
        }
    }

    pub fn ui(&self) -> &'a egui::Ui {
        self.ui.get()
    }

    pub fn painter(&self) -> &'a egui::Painter {
        self.painter.get()
    }

    pub fn node_width(&self, node_id: NodeId) -> f32 {
        self.node_widths
            .get(&node_id)
            .copied()
            .expect("node width must be precomputed")
    }

    pub fn node_rect(
        &self,
        view_node: &model::ViewNode,
        input_count: usize,
        output_count: usize,
    ) -> egui::Rect {
        node::node_rect_for_graph(
            self.origin,
            view_node,
            input_count,
            output_count,
            self.scale,
            &self.layout,
            self.node_width(view_node.id),
        )
    }
}

pub trait WidgetRenderer {
    type Output;

    fn render(
        &mut self,
        ctx: &RenderContext,
        view_graph: &mut model::ViewGraph,
        func_lib: &FuncLib,
    ) -> Self::Output;
}
