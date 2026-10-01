//! Taffy-powered CSS Flexbox and Grid layout engine for Kiln Native GUI (ADR 0074).
//!
//! Provides deterministic, web-standard layout calculation across macOS, Linux, and Windows.

use std::collections::HashMap;
use std::sync::Mutex;
use taffy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub struct KilnLayoutEngine {
    tree: TaffyTree<()>,
    widget_nodes: HashMap<u64, NodeId>,
    node_widgets: HashMap<NodeId, u64>,
}

// Invariant (ADR 0074): Kiln GUI layout is pinned to Thread 0.
unsafe impl Send for KilnLayoutEngine {}
unsafe impl Sync for KilnLayoutEngine {}

impl Default for KilnLayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl KilnLayoutEngine {
    pub fn new() -> Self {
        Self {
            tree: TaffyTree::new(),
            widget_nodes: HashMap::new(),
            node_widgets: HashMap::new(),
        }
    }

    pub fn get_or_create_node(&mut self, widget_id: u64, style: Style) -> NodeId {
        if let Some(&node_id) = self.widget_nodes.get(&widget_id) {
            let _ = self.tree.set_style(node_id, style);
            node_id
        } else {
            let node_id = self.tree.new_leaf(style).expect("create taffy leaf");
            self.widget_nodes.insert(widget_id, node_id);
            self.node_widgets.insert(node_id, widget_id);
            node_id
        }
    }

    pub fn set_node_style(&mut self, widget_id: u64, style: Style) {
        if let Some(&node_id) = self.widget_nodes.get(&widget_id) {
            let _ = self.tree.set_style(node_id, style);
        } else {
            self.get_or_create_node(widget_id, style);
        }
    }

    pub fn update_node_style(&mut self, widget_id: u64, f: impl FnOnce(&mut Style)) {
        if let Some(&node_id) = self.widget_nodes.get(&widget_id) {
            if let Ok(mut style) = self.tree.style(node_id).cloned() {
                f(&mut style);
                let _ = self.tree.set_style(node_id, style);
            }
        }
    }

    pub fn add_child(&mut self, parent_widget_id: u64, child_widget_id: u64) {
        if let (Some(&parent_node), Some(&child_node)) = (
            self.widget_nodes.get(&parent_widget_id),
            self.widget_nodes.get(&child_widget_id),
        ) {
            // Avoid duplicate additions
            if let Ok(children) = self.tree.children(parent_node) {
                if !children.contains(&child_node) {
                    let _ = self.tree.add_child(parent_node, child_node);
                }
            } else {
                let _ = self.tree.add_child(parent_node, child_node);
            }
        }
    }

    pub fn remove_child(&mut self, parent_widget_id: u64, child_widget_id: u64) {
        if let (Some(&parent_node), Some(&child_node)) = (
            self.widget_nodes.get(&parent_widget_id),
            self.widget_nodes.get(&child_widget_id),
        ) {
            let _ = self.tree.remove_child(parent_node, child_node);
        }
    }

    pub fn remove_node(&mut self, widget_id: u64) {
        if let Some(node_id) = self.widget_nodes.remove(&widget_id) {
            self.node_widgets.remove(&node_id);
            let _ = self.tree.remove(node_id);
        }
    }

    pub fn compute_layout(
        &mut self,
        root_widget_id: u64,
        available_width: f32,
        available_height: f32,
    ) {
        if let Some(&root_node) = self.widget_nodes.get(&root_widget_id) {
            let space = Size {
                width: if available_width > 0.0 {
                    AvailableSpace::Definite(available_width)
                } else {
                    AvailableSpace::MinContent
                },
                height: if available_height > 0.0 {
                    AvailableSpace::Definite(available_height)
                } else {
                    AvailableSpace::MinContent
                },
            };
            let _ = self.tree.compute_layout(root_node, space);
        }
    }

    pub fn get_layout(&self, widget_id: u64) -> Option<LayoutRect> {
        let node_id = self.widget_nodes.get(&widget_id)?;
        let layout = self.tree.layout(*node_id).ok()?;
        Some(LayoutRect {
            x: layout.location.x,
            y: layout.location.y,
            width: layout.size.width,
            height: layout.size.height,
        })
    }

    pub fn get_child_layouts(&self, parent_widget_id: u64) -> Vec<(u64, LayoutRect)> {
        let mut results = Vec::new();
        if let Some(&parent_node) = self.widget_nodes.get(&parent_widget_id) {
            if let Ok(children) = self.tree.children(parent_node) {
                for child_node in children {
                    if let Some(&child_widget_id) = self.node_widgets.get(&child_node) {
                        if let Ok(layout) = self.tree.layout(child_node) {
                            results.push((
                                child_widget_id,
                                LayoutRect {
                                    x: layout.location.x,
                                    y: layout.location.y,
                                    width: layout.size.width,
                                    height: layout.size.height,
                                },
                            ));
                        }
                    }
                }
            }
        }
        results
    }
}

// Style parsing utilities
pub fn parse_dimension(val: &serde_json::Value) -> Dimension {
    match val {
        serde_json::Value::Number(n) => length(n.as_f64().unwrap_or(0.0) as f32),
        serde_json::Value::String(s) => {
            let s = s.trim().to_lowercase();
            if s == "auto" {
                auto()
            } else if let Some(pct) = s.strip_suffix('%') {
                if let Ok(num) = pct.trim().parse::<f32>() {
                    percent(num / 100.0)
                } else {
                    auto()
                }
            } else if let Some(px) = s.strip_suffix("px") {
                if let Ok(num) = px.trim().parse::<f32>() {
                    length(num)
                } else {
                    auto()
                }
            } else if let Ok(num) = s.parse::<f32>() {
                length(num)
            } else {
                auto()
            }
        }
        _ => auto(),
    }
}

pub fn parse_length_percentage(val: &serde_json::Value) -> LengthPercentage {
    match val {
        serde_json::Value::Number(n) => length(n.as_f64().unwrap_or(0.0) as f32),
        serde_json::Value::String(s) => {
            let s = s.trim().to_lowercase();
            if let Some(pct) = s.strip_suffix('%') {
                if let Ok(num) = pct.trim().parse::<f32>() {
                    percent(num / 100.0)
                } else {
                    length(0.0)
                }
            } else if let Some(px) = s.strip_suffix("px") {
                if let Ok(num) = px.trim().parse::<f32>() {
                    length(num)
                } else {
                    length(0.0)
                }
            } else if let Ok(num) = s.parse::<f32>() {
                length(num)
            } else {
                length(0.0)
            }
        }
        _ => length(0.0),
    }
}

pub fn parse_length_percentage_auto(val: &serde_json::Value) -> LengthPercentageAuto {
    match val {
        serde_json::Value::Number(n) => length(n.as_f64().unwrap_or(0.0) as f32),
        serde_json::Value::String(s) => {
            let s = s.trim().to_lowercase();
            if s == "auto" {
                auto()
            } else if let Some(pct) = s.strip_suffix('%') {
                if let Ok(num) = pct.trim().parse::<f32>() {
                    percent(num / 100.0)
                } else {
                    auto()
                }
            } else if let Some(px) = s.strip_suffix("px") {
                if let Ok(num) = px.trim().parse::<f32>() {
                    length(num)
                } else {
                    auto()
                }
            } else if let Ok(num) = s.parse::<f32>() {
                length(num)
            } else {
                auto()
            }
        }
        _ => auto(),
    }
}

fn parse_single_grid_track(s: &str) -> Option<GridTemplateComponent<String>> {
    let s = s.trim().to_lowercase();
    if s == "auto" {
        Some(auto())
    } else if let Some(fr_val) = s.strip_suffix("fr") {
        let n = fr_val.trim().parse::<f32>().ok()?;
        Some(fr(n))
    } else if let Some(pct) = s.strip_suffix('%') {
        let n = pct.trim().parse::<f32>().ok()?;
        Some(percent(n / 100.0))
    } else if let Some(px) = s.strip_suffix("px") {
        let n = px.trim().parse::<f32>().ok()?;
        Some(length(n))
    } else if let Ok(n) = s.parse::<f32>() {
        Some(length(n))
    } else {
        None
    }
}

pub fn parse_grid_template_tracks(s: &str) -> Vec<GridTemplateComponent<String>> {
    let mut tracks = Vec::new();
    let s = s.trim();
    let mut rest = s;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.starts_with("repeat(") {
            if let Some(end_paren) = rest.find(')') {
                let inside = &rest["repeat(".len()..end_paren];
                let parts: Vec<&str> = inside.split(',').collect();
                if parts.len() == 2 {
                    if let (Ok(count), Some(track)) = (
                        parts[0].trim().parse::<usize>(),
                        parse_single_grid_track(parts[1]),
                    ) {
                        for _ in 0..count {
                            tracks.push(track.clone());
                        }
                    }
                }
                rest = &rest[end_paren + 1..];
                continue;
            }
        }
        let token_end = rest
            .find(|c: char| c.is_whitespace() || c == ',')
            .unwrap_or(rest.len());
        let token = &rest[..token_end];
        if let Some(track) = parse_single_grid_track(token) {
            tracks.push(track);
        }
        rest = &rest[token_end..];
    }
    tracks
}

pub fn parse_style(val: &serde_json::Value) -> Style {
    let mut style = Style::default();
    let obj = match val.as_object() {
        Some(o) => o,
        None => return style,
    };

    if let Some(d) = obj.get("display").and_then(|v| v.as_str()) {
        style.display = match d.to_lowercase().as_str() {
            "flex" => Display::Flex,
            "grid" => Display::Grid,
            "none" => Display::None,
            _ => Display::Flex,
        };
    }

    if let Some(fd) = obj
        .get("flexDirection")
        .or_else(|| obj.get("flex_direction"))
        .and_then(|v| v.as_str())
    {
        style.flex_direction = match fd.to_lowercase().as_str() {
            "row" => FlexDirection::Row,
            "column" => FlexDirection::Column,
            "row-reverse" => FlexDirection::RowReverse,
            "column-reverse" => FlexDirection::ColumnReverse,
            _ => FlexDirection::Row,
        };
    }

    if let Some(fw) = obj
        .get("flexWrap")
        .or_else(|| obj.get("flex_wrap"))
        .and_then(|v| v.as_str())
    {
        style.flex_wrap = match fw.to_lowercase().as_str() {
            "wrap" => FlexWrap::Wrap,
            "wrap-reverse" => FlexWrap::WrapReverse,
            _ => FlexWrap::NoWrap,
        };
    }

    if let Some(jc) = obj
        .get("justifyContent")
        .or_else(|| obj.get("justify_content"))
        .and_then(|v| v.as_str())
    {
        style.justify_content = match jc.to_lowercase().as_str() {
            "flex-start" | "start" => Some(JustifyContent::START),
            "flex-end" | "end" => Some(JustifyContent::END),
            "center" => Some(JustifyContent::CENTER),
            "space-between" => Some(JustifyContent::SPACE_BETWEEN),
            "space-around" => Some(JustifyContent::SPACE_AROUND),
            "space-evenly" => Some(JustifyContent::SPACE_EVENLY),
            _ => None,
        };
    }

    if let Some(ai) = obj
        .get("alignItems")
        .or_else(|| obj.get("align_items"))
        .and_then(|v| v.as_str())
    {
        style.align_items = match ai.to_lowercase().as_str() {
            "flex-start" | "start" => Some(AlignItems::START),
            "flex-end" | "end" => Some(AlignItems::END),
            "center" => Some(AlignItems::CENTER),
            "baseline" => Some(AlignItems::BASELINE),
            "stretch" => Some(AlignItems::STRETCH),
            _ => None,
        };
    }

    if let Some(as_val) = obj
        .get("alignSelf")
        .or_else(|| obj.get("align_self"))
        .and_then(|v| v.as_str())
    {
        style.align_self = match as_val.to_lowercase().as_str() {
            "flex-start" | "start" => Some(AlignSelf::START),
            "flex-end" | "end" => Some(AlignSelf::END),
            "center" => Some(AlignSelf::CENTER),
            "baseline" => Some(AlignSelf::BASELINE),
            "stretch" => Some(AlignSelf::STRETCH),
            _ => None,
        };
    }

    // Gap
    if let Some(g) = obj.get("gap") {
        let lp = parse_length_percentage(g);
        style.gap = Size {
            width: lp,
            height: lp,
        };
    }
    if let Some(rg) = obj.get("rowGap").or_else(|| obj.get("row_gap")) {
        style.gap.height = parse_length_percentage(rg);
    }
    if let Some(cg) = obj.get("columnGap").or_else(|| obj.get("column_gap")) {
        style.gap.width = parse_length_percentage(cg);
    }

    // Padding
    if let Some(p) = obj.get("padding") {
        if let Some(arr) = p.as_array() {
            if arr.len() == 4 {
                style.padding = Rect {
                    top: parse_length_percentage(&arr[0]),
                    right: parse_length_percentage(&arr[1]),
                    bottom: parse_length_percentage(&arr[2]),
                    left: parse_length_percentage(&arr[3]),
                };
            } else if arr.len() == 2 {
                let v = parse_length_percentage(&arr[0]);
                let h = parse_length_percentage(&arr[1]);
                style.padding = Rect {
                    top: v,
                    bottom: v,
                    left: h,
                    right: h,
                };
            }
        } else {
            let lp = parse_length_percentage(p);
            style.padding = Rect {
                left: lp,
                right: lp,
                top: lp,
                bottom: lp,
            };
        }
    }

    // Margin
    if let Some(m) = obj.get("margin") {
        let lpa = parse_length_percentage_auto(m);
        style.margin = Rect {
            left: lpa,
            right: lpa,
            top: lpa,
            bottom: lpa,
        };
    }

    // Sizing
    if let Some(w) = obj.get("width") {
        style.size.width = parse_dimension(w);
    }
    if let Some(h) = obj.get("height") {
        style.size.height = parse_dimension(h);
    }
    if let Some(mw) = obj.get("minWidth").or_else(|| obj.get("min_width")) {
        style.min_size.width = parse_length_percentage_auto(mw);
    }
    if let Some(mh) = obj.get("minHeight").or_else(|| obj.get("min_height")) {
        style.min_size.height = parse_length_percentage_auto(mh);
    }
    if let Some(mw) = obj.get("maxWidth").or_else(|| obj.get("max_width")) {
        style.max_size.width = parse_length_percentage_auto(mw);
    }
    if let Some(mh) = obj.get("maxHeight").or_else(|| obj.get("max_height")) {
        style.max_size.height = parse_length_percentage_auto(mh);
    }

    // Flex properties
    if let Some(fg) = obj.get("flexGrow").or_else(|| obj.get("flex_grow")) {
        if let Some(n) = fg.as_f64() {
            style.flex_grow = n as f32;
        }
    }
    if let Some(fs) = obj.get("flexShrink").or_else(|| obj.get("flex_shrink")) {
        if let Some(n) = fs.as_f64() {
            style.flex_shrink = n as f32;
        }
    }
    if let Some(fb) = obj.get("flexBasis").or_else(|| obj.get("flex_basis")) {
        style.flex_basis = parse_dimension(fb);
    }

    // CSS Grid Template Columns / Rows
    if let Some(gtc) = obj
        .get("gridTemplateColumns")
        .or_else(|| obj.get("grid_template_columns"))
        .and_then(|v| v.as_str())
    {
        style.grid_template_columns = parse_grid_template_tracks(gtc);
    }
    if let Some(gtr) = obj
        .get("gridTemplateRows")
        .or_else(|| obj.get("grid_template_rows"))
        .and_then(|v| v.as_str())
    {
        style.grid_template_rows = parse_grid_template_tracks(gtr);
    }

    style
}

static GLOBAL_LAYOUT_ENGINE: Mutex<Option<KilnLayoutEngine>> = Mutex::new(None);

pub fn with_layout_engine<R>(f: impl FnOnce(&mut KilnLayoutEngine) -> R) -> R {
    let mut guard = GLOBAL_LAYOUT_ENGINE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(KilnLayoutEngine::new());
    }
    f(guard.as_mut().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flexbox_layout() {
        let mut engine = KilnLayoutEngine::new();
        let root_style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            gap: Size {
                width: length(10.0),
                height: length(10.0),
            },
            size: Size {
                width: length(300.0),
                height: length(100.0),
            },
            ..Default::default()
        };
        let child_style = Style {
            size: Size {
                width: length(50.0),
                height: length(50.0),
            },
            ..Default::default()
        };

        engine.get_or_create_node(1, root_style);
        engine.get_or_create_node(2, child_style.clone());
        engine.get_or_create_node(3, child_style);

        engine.add_child(1, 2);
        engine.add_child(1, 3);

        engine.compute_layout(1, 300.0, 100.0);

        let layout2 = engine.get_layout(2).unwrap();
        let layout3 = engine.get_layout(3).unwrap();

        assert_eq!(layout2.x, 0.0);
        assert_eq!(layout2.width, 50.0);
        assert_eq!(layout3.x, 60.0); // 50 + 10 gap
        assert_eq!(layout3.width, 50.0);

        let child_layouts = engine.get_child_layouts(1);
        assert_eq!(child_layouts.len(), 2);
        assert_eq!(child_layouts[0].0, 2);
        assert_eq!(child_layouts[1].0, 3);
    }

    #[test]
    fn test_grid_layout() {
        let mut engine = KilnLayoutEngine::new();
        let root_style = Style {
            display: Display::Grid,
            grid_template_columns: vec![length(100.0), fr(1.0)],
            gap: Size {
                width: length(10.0),
                height: length(10.0),
            },
            size: Size {
                width: length(310.0),
                height: length(100.0),
            },
            ..Default::default()
        };
        let child_style = Style {
            size: Size {
                width: auto(),
                height: length(50.0),
            },
            ..Default::default()
        };

        engine.get_or_create_node(10, root_style);
        engine.get_or_create_node(11, child_style.clone());
        engine.get_or_create_node(12, child_style);

        engine.add_child(10, 11);
        engine.add_child(10, 12);

        // Available 310 width - 10 gap = 300 track space.
        // length(100) + fr(1.0) = 100 + 200 = 300.
        engine.compute_layout(10, 310.0, 100.0);

        let layout1 = engine.get_layout(11).unwrap();
        let layout2 = engine.get_layout(12).unwrap();

        assert_eq!(layout1.x, 0.0);
        assert_eq!(layout1.width, 100.0);
        assert_eq!(layout2.x, 110.0); // 100 + 10 gap
        assert_eq!(layout2.width, 200.0);
    }

    #[test]
    fn test_style_json_parser() {
        let json: serde_json::Value = serde_json::json!({
            "display": "grid",
            "gridTemplateColumns": "repeat(3, 1fr)",
            "gap": 16,
            "padding": 24,
            "alignItems": "center"
        });

        let style = parse_style(&json);
        assert_eq!(style.display, Display::Grid);
        assert_eq!(style.grid_template_columns.len(), 3);
        assert_eq!(style.gap.width, length(16.0));
        assert_eq!(style.gap.height, length(16.0));
        assert_eq!(style.padding.left, length(24.0));
        assert_eq!(style.align_items, Some(AlignItems::CENTER));
    }
}
