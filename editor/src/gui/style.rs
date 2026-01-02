use eframe::egui;
use egui::{Color32, Stroke};

const STROKE_WIDTH: f32 = 1.0;
const HEADER_TEXT_OFFSET: f32 = 4.0;
const CACHE_BUTTON_WIDTH_FACTOR: f32 = 3.1;
const CACHE_BUTTON_VERTICAL_PAD_FACTOR: f32 = 0.4;
const CACHE_BUTTON_TEXT_PAD_FACTOR: f32 = 0.5;
const NODE_FILL: Color32 = Color32::from_rgb(27, 27, 27);
const CACHE_ACTIVE_COLOR: Color32 = Color32::from_rgb(240, 205, 90);
const CACHE_CHECKED_TEXT_COLOR: Color32 = Color32::from_rgb(60, 50, 20);
const STATUS_DOT_RADIUS: f32 = 4.0;
const STATUS_ITEM_GAP: f32 = 6.0;
const INPUT_PORT_COLOR: Color32 = Color32::from_rgb(70, 150, 255);
const CONST_STROKE_COLOR: Color32 = Color32::from_rgb(70, 150, 255);
const OUTPUT_PORT_COLOR: Color32 = Color32::from_rgb(70, 200, 200);
const INPUT_HOVER_COLOR: Color32 = Color32::from_rgb(120, 190, 255);
const OUTPUT_HOVER_COLOR: Color32 = Color32::from_rgb(110, 230, 210);
const CONNECTION_STROKE_WIDTH: f32 = 2.0;
const CONNECTION_HIGHLIGHT_COLOR: Color32 = Color32::from_rgb(255, 90, 90);
const TEMP_CONNECTION_COLOR: Color32 = Color32::from_rgb(170, 200, 255);
const BREAKER_COLOR: Color32 = Color32::from_rgb(255, 120, 120);
const SELECTED_STROKE_COLOR: Color32 = Color32::from_rgb(192, 222, 255);
const NODE_STROKE_COLOR: Color32 = Color32::from_rgb(60, 60, 60);
const DOTTED_COLOR: Color32 = Color32::from_rgb(48, 48, 48);
const DOTTED_BASE_SPACING: f32 = 24.0;
const DOTTED_RADIUS_BASE: f32 = 1.2;
const DOTTED_RADIUS_MIN: f32 = 0.6;
const DOTTED_RADIUS_MAX: f32 = 2.4;
const CONST_STROKE_GAMMA: f32 = 0.7;

#[derive(Debug, Clone)]
pub struct Style {
    pub header_text_offset: f32,
    pub cache_button_width_factor: f32,
    pub cache_button_vertical_pad_factor: f32,
    pub cache_button_text_pad_factor: f32,
    pub cache_active_color: Color32,
    pub cache_checked_text_color: Color32,
    pub status_dot_radius: f32,
    pub status_item_gap: f32,
    pub input_port_color: Color32,
    pub output_port_color: Color32,
    pub input_hover_color: Color32,
    pub output_hover_color: Color32,
    pub connection_stroke: Stroke,
    pub connection_highlight_stroke: Stroke,
    pub temp_connection_stroke: Stroke,
    pub breaker_stroke: Stroke,
    pub dotted_color: Color32,
    pub dotted_base_spacing: f32,
    pub dotted_radius_base: f32,
    pub dotted_radius_min: f32,
    pub dotted_radius_max: f32,
    pub node_fill: Color32,
    pub node_stroke: Stroke,
    pub const_stroke: Stroke,
    pub selected_stroke: Stroke,
}

impl Style {
    pub fn new() -> Self {
        Self {
            header_text_offset: HEADER_TEXT_OFFSET,
            cache_button_width_factor: CACHE_BUTTON_WIDTH_FACTOR,
            cache_button_vertical_pad_factor: CACHE_BUTTON_VERTICAL_PAD_FACTOR,
            cache_button_text_pad_factor: CACHE_BUTTON_TEXT_PAD_FACTOR,
            cache_active_color: CACHE_ACTIVE_COLOR,
            cache_checked_text_color: CACHE_CHECKED_TEXT_COLOR,
            status_dot_radius: STATUS_DOT_RADIUS,
            status_item_gap: STATUS_ITEM_GAP,
            input_port_color: INPUT_PORT_COLOR,
            output_port_color: OUTPUT_PORT_COLOR,
            input_hover_color: INPUT_HOVER_COLOR,
            output_hover_color: OUTPUT_HOVER_COLOR,
            connection_stroke: Stroke::new(CONNECTION_STROKE_WIDTH, INPUT_PORT_COLOR),
            connection_highlight_stroke: Stroke::new(
                CONNECTION_STROKE_WIDTH,
                CONNECTION_HIGHLIGHT_COLOR,
            ),
            temp_connection_stroke: Stroke::new(CONNECTION_STROKE_WIDTH, TEMP_CONNECTION_COLOR),
            breaker_stroke: Stroke::new(CONNECTION_STROKE_WIDTH, BREAKER_COLOR),
            dotted_color: DOTTED_COLOR,
            dotted_base_spacing: DOTTED_BASE_SPACING,
            dotted_radius_base: DOTTED_RADIUS_BASE,
            dotted_radius_min: DOTTED_RADIUS_MIN,
            dotted_radius_max: DOTTED_RADIUS_MAX,
            node_fill: NODE_FILL,
            node_stroke: Stroke::new(STROKE_WIDTH, NODE_STROKE_COLOR),
            selected_stroke: Stroke::new(STROKE_WIDTH, SELECTED_STROKE_COLOR),
            const_stroke: Stroke::new(STROKE_WIDTH, CONST_STROKE_COLOR),
        }
    }
}
