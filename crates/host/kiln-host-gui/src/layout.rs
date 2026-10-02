//! Layout for kiln:gui (ADR 0074): one flexbox/grid model for every container, computed by Taffy.
//!
//! The TypeScript side owns layout *semantics*: it turns a widget's tag defaults, its `style`
//! object and its individual props into one style object and sends it as the widget's `layout`
//! property. This module parses that object into a Taffy [`Style`] and keeps the tree. Platform
//! backends own *measurement*: a leaf (a label, a button) is measured by the toolkit through the
//! closure handed to [`Layouts::compute`].

use std::collections::HashMap;

use serde_json::Value;
use taffy::prelude::*;

/// A frame relative to the parent's origin, in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The layout tree, keyed by host widget handle. `C` is the backend's measurement context for
/// leaves; containers carry none.
pub struct Layouts<C> {
    tree: TaffyTree<C>,
    nodes: HashMap<u32, NodeId>,
    handles: HashMap<NodeId, u32>,
}

impl<C> Default for Layouts<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> Layouts<C> {
    pub fn new() -> Self {
        Self {
            tree: TaffyTree::new(),
            nodes: HashMap::new(),
            handles: HashMap::new(),
        }
    }

    /// Registers `handle` with its default style; `context` is present for measured leaves.
    pub fn create(&mut self, handle: u32, style: Style, context: Option<C>) {
        let node = match context {
            Some(context) => self.tree.new_leaf_with_context(style, context),
            None => self.tree.new_leaf(style),
        }
        .expect("taffy node allocation cannot fail");
        self.nodes.insert(handle, node);
        self.handles.insert(node, handle);
    }

    pub fn set_style(&mut self, handle: u32, style: Style) {
        if let Some(&node) = self.nodes.get(&handle) {
            let _ = self.tree.set_style(node, style);
        }
    }

    /// The measured content of a leaf changed (new text, new font): drop its cached sizes.
    pub fn mark_dirty(&mut self, handle: u32) {
        if let Some(&node) = self.nodes.get(&handle) {
            let _ = self.tree.mark_dirty(node);
        }
    }

    /// Inserts `child` under `parent` before `before`, or last when `before` is absent or not a
    /// child of `parent`. Re-inserting an existing child moves it.
    pub fn insert(&mut self, parent: u32, child: u32, before: Option<u32>) {
        let (Some(&parent_node), Some(&child_node)) =
            (self.nodes.get(&parent), self.nodes.get(&child))
        else {
            return;
        };
        if let Some(old_parent) = self.tree.parent(child_node) {
            let _ = self.tree.remove_child(old_parent, child_node);
        }
        let children = self.tree.children(parent_node).unwrap_or_default();
        let index = before
            .and_then(|b| self.nodes.get(&b))
            .and_then(|b| children.iter().position(|c| c == b))
            .unwrap_or(children.len());
        let _ = self
            .tree
            .insert_child_at_index(parent_node, index, child_node);
    }

    pub fn remove(&mut self, parent: u32, child: u32) {
        if let (Some(&parent_node), Some(&child_node)) =
            (self.nodes.get(&parent), self.nodes.get(&child))
        {
            let _ = self.tree.remove_child(parent_node, child_node);
        }
    }

    /// Forgets `handle`. Its children stay registered under their own handles; the backend
    /// destroys them separately.
    pub fn drop_node(&mut self, handle: u32) {
        if let Some(node) = self.nodes.remove(&handle) {
            self.handles.remove(&node);
            let _ = self.tree.remove(node);
        }
    }

    pub fn children(&self, handle: u32) -> Vec<u32> {
        self.nodes
            .get(&handle)
            .and_then(|&node| self.tree.children(node).ok())
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| self.handles.get(n).copied())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Lays out the subtree rooted at `root`. `None` for a dimension means "as large as the
    /// content wants" (a scroll view's document height).
    pub fn compute(
        &mut self,
        root: u32,
        width: Option<f64>,
        height: Option<f64>,
        mut measure: impl FnMut(&mut C, Size<Option<f32>>, Size<AvailableSpace>) -> Size<f32>,
    ) {
        let Some(&root_node) = self.nodes.get(&root) else {
            return;
        };
        // The root fills the space it is given, whatever its own style says: a window's content
        // is the window, a scroll document is as wide as the visible area.
        if let Ok(current) = self.tree.style(root_node) {
            let mut style = current.clone();
            if let Some(w) = width {
                style.size.width = length(w.max(0.0) as f32);
            }
            if let Some(h) = height {
                style.size.height = length(h.max(0.0) as f32);
            }
            if style.size != current.size {
                let _ = self.tree.set_style(root_node, style);
            }
        }
        let space = |v: Option<f64>| match v {
            Some(v) => AvailableSpace::Definite(v.max(0.0) as f32),
            None => AvailableSpace::MaxContent,
        };
        let available = Size {
            width: space(width),
            height: space(height),
        };
        let _ = self.tree.compute_layout_with_measure(
            root_node,
            available,
            |inputs, _, context, style| {
                taffy::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, available| match context {
                        Some(context) => {
                            // A dimension the parent already decided (a stretched width) is final.
                            let measured = measure(context, known, available);
                            Size {
                                width: known.width.unwrap_or(measured.width),
                                height: known.height.unwrap_or(measured.height),
                            }
                        }
                        None => Size::ZERO,
                    },
                )
            },
        );
    }

    pub fn frame(&self, handle: u32) -> Option<Frame> {
        let layout = self.tree.layout(*self.nodes.get(&handle)?).ok()?;
        Some(Frame {
            x: layout.location.x as f64,
            y: layout.location.y as f64,
            width: layout.size.width as f64,
            height: layout.size.height as f64,
        })
    }

    /// The size of the content inside `handle` (for a scroll document: how tall it really is).
    pub fn content_size(&self, handle: u32) -> Option<(f64, f64)> {
        let layout = self.tree.layout(*self.nodes.get(&handle)?).ok()?;
        Some((layout.size.width as f64, layout.size.height as f64))
    }
}

// ---------------------------------------------------------------------------------------------
// Style parsing. Accepts the camelCase names TypeScript sends; numbers are points, strings may
// be "auto", "50%" or "12px".

fn number(value: &Value) -> Option<f32> {
    match value {
        Value::Number(n) => n.as_f64().map(|v| v as f32),
        Value::String(s) => s.trim().trim_end_matches("px").trim().parse().ok(),
        _ => None,
    }
}

fn percentage(value: &Value) -> Option<f32> {
    value
        .as_str()
        .and_then(|s| s.trim().strip_suffix('%'))
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|p| p / 100.0)
}

fn is_auto(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("auto"))
}

pub fn parse_dimension(value: &Value) -> Dimension {
    if let Some(p) = percentage(value) {
        percent(p)
    } else if let Some(n) = number(value) {
        length(n)
    } else {
        auto()
    }
}

pub fn parse_length_percentage(value: &Value) -> LengthPercentage {
    if let Some(p) = percentage(value) {
        percent(p)
    } else {
        length(number(value).unwrap_or(0.0))
    }
}

pub fn parse_length_percentage_auto(value: &Value) -> LengthPercentageAuto {
    if is_auto(value) {
        auto()
    } else if let Some(p) = percentage(value) {
        percent(p)
    } else if let Some(n) = number(value) {
        length(n)
    } else {
        auto()
    }
}

/// `12`, `[vertical, horizontal]`, `[top, right, bottom, left]` or `{ top, right, bottom, left }`.
fn parse_edges<T: Copy>(value: &Value, zero: T, parse: impl Fn(&Value) -> T) -> Rect<T> {
    match value {
        Value::Array(items) if items.len() == 4 => Rect {
            top: parse(&items[0]),
            right: parse(&items[1]),
            bottom: parse(&items[2]),
            left: parse(&items[3]),
        },
        Value::Array(items) if items.len() == 2 => {
            let (v, h) = (parse(&items[0]), parse(&items[1]));
            Rect {
                top: v,
                bottom: v,
                left: h,
                right: h,
            }
        }
        Value::Object(map) => {
            let side = |name: &str| map.get(name).map(&parse).unwrap_or(zero);
            Rect {
                top: side("top"),
                right: side("right"),
                bottom: side("bottom"),
                left: side("left"),
            }
        }
        other => {
            let v = parse(other);
            Rect {
                top: v,
                right: v,
                bottom: v,
                left: v,
            }
        }
    }
}

fn parse_track(token: &str) -> Option<GridTemplateComponent<String>> {
    let token = token.trim().to_ascii_lowercase();
    if token == "auto" {
        Some(auto())
    } else if let Some(n) = token.strip_suffix("fr") {
        n.trim().parse::<f32>().ok().map(fr)
    } else if let Some(p) = token.strip_suffix('%') {
        p.trim().parse::<f32>().ok().map(|p| percent(p / 100.0))
    } else if let Some(px) = token.strip_suffix("px") {
        px.trim().parse::<f32>().ok().map(length)
    } else {
        token.parse::<f32>().ok().map(length)
    }
}

/// `"1fr 2fr"`, `"200px auto"`, `"repeat(3, 1fr)"`, or a number of equal columns (`3`).
pub fn parse_grid_template_tracks(spec: &str) -> Vec<GridTemplateComponent<String>> {
    if let Ok(count) = spec.trim().parse::<usize>() {
        return (0..count).map(|_| fr(1.0)).collect();
    }
    let mut tracks = Vec::new();
    let mut rest = spec.trim();
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix("repeat(") {
            let Some(end) = inner.find(')') else { break };
            if let Some((count, track)) = inner[..end].split_once(',')
                && let (Ok(count), Some(track)) =
                    (count.trim().parse::<usize>(), parse_track(track))
            {
                tracks.extend(std::iter::repeat_n(track, count));
            }
            rest = inner[end + 1..].trim_start();
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || c == ',')
            .unwrap_or(rest.len());
        if let Some(track) = parse_track(&rest[..end]) {
            tracks.push(track);
        }
        rest = rest[end..].trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    }
    tracks
}

fn parse_tracks(value: &Value) -> Vec<GridTemplateComponent<String>> {
    match value {
        Value::Number(n) => parse_grid_template_tracks(&n.to_string()),
        Value::String(s) => parse_grid_template_tracks(s),
        _ => Vec::new(),
    }
}

/// `2` (start line), `"span 2"`, `"1 / 3"`, `"2 / span 2"`.
pub fn parse_grid_placement(value: &Value) -> Line<GridPlacement<String>> {
    fn one(token: &str) -> GridPlacement<String> {
        let token = token.trim();
        if let Some(n) = token.strip_prefix("span") {
            n.trim()
                .parse::<u16>()
                .map(GridPlacement::Span)
                .unwrap_or_default()
        } else {
            token.parse::<i16>().map(line).unwrap_or_default()
        }
    }
    match value {
        Value::Number(n) => Line {
            start: line(n.as_i64().unwrap_or(1) as i16),
            end: GridPlacement::Auto,
        },
        Value::String(s) => match s.split_once('/') {
            Some((start, end)) => Line {
                start: one(start),
                end: one(end),
            },
            None => Line {
                start: one(s),
                end: GridPlacement::Auto,
            },
        },
        _ => Line {
            start: GridPlacement::Auto,
            end: GridPlacement::Auto,
        },
    }
}

fn align_items(value: &str) -> Option<AlignItems> {
    match value {
        "flex-start" | "start" | "leading" | "top" => Some(AlignItems::START),
        "flex-end" | "end" | "trailing" | "bottom" => Some(AlignItems::END),
        "center" => Some(AlignItems::CENTER),
        "baseline" => Some(AlignItems::BASELINE),
        "stretch" => Some(AlignItems::STRETCH),
        _ => None,
    }
}

fn align_content(value: &str) -> Option<AlignContent> {
    match value {
        "flex-start" | "start" => Some(AlignContent::START),
        "flex-end" | "end" => Some(AlignContent::END),
        "center" => Some(AlignContent::CENTER),
        "stretch" => Some(AlignContent::STRETCH),
        "space-between" => Some(AlignContent::SPACE_BETWEEN),
        "space-around" => Some(AlignContent::SPACE_AROUND),
        "space-evenly" => Some(AlignContent::SPACE_EVENLY),
        _ => None,
    }
}

pub fn parse_style(value: &Value) -> Style {
    let mut style = Style::default();
    let Some(obj) = value.as_object() else {
        return style;
    };
    let text = |name: &str| {
        obj.get(name)
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
    };

    if let Some(display) = text("display") {
        style.display = match display.as_str() {
            "grid" => Display::Grid,
            "none" => Display::None,
            _ => Display::Flex,
        };
    }
    if let Some(direction) = text("flexDirection") {
        style.flex_direction = match direction.as_str() {
            "column" => FlexDirection::Column,
            "row-reverse" => FlexDirection::RowReverse,
            "column-reverse" => FlexDirection::ColumnReverse,
            _ => FlexDirection::Row,
        };
    }
    if let Some(wrap) = text("flexWrap") {
        style.flex_wrap = match wrap.as_str() {
            "wrap" => FlexWrap::Wrap,
            "wrap-reverse" => FlexWrap::WrapReverse,
            _ => FlexWrap::NoWrap,
        };
    }
    if let Some(v) = text("justifyContent") {
        style.justify_content = align_content(&v);
    }
    if let Some(v) = text("alignContent") {
        style.align_content = align_content(&v);
    }
    if let Some(v) = text("alignItems") {
        style.align_items = align_items(&v);
    }
    if let Some(v) = text("justifyItems") {
        style.justify_items = align_items(&v);
    }
    if let Some(v) = text("alignSelf") {
        style.align_self = align_items(&v);
    }
    if let Some(v) = text("justifySelf") {
        style.justify_self = align_items(&v);
    }
    if let Some(v) = text("position") {
        style.position = if v == "absolute" {
            Position::Absolute
        } else {
            Position::Relative
        };
    }

    if let Some(gap) = obj.get("gap") {
        let gap = parse_length_percentage(gap);
        style.gap = Size {
            width: gap,
            height: gap,
        };
    }
    if let Some(v) = obj.get("rowGap") {
        style.gap.height = parse_length_percentage(v);
    }
    if let Some(v) = obj.get("columnGap") {
        style.gap.width = parse_length_percentage(v);
    }
    if let Some(v) = obj.get("padding") {
        style.padding = parse_edges(v, length(0.0), parse_length_percentage);
    }
    if let Some(v) = obj.get("margin") {
        style.margin = parse_edges(v, length(0.0), parse_length_percentage_auto);
    }
    for (name, side) in [("top", 0), ("right", 1), ("bottom", 2), ("left", 3)] {
        if let Some(v) = obj.get(name) {
            let v = parse_length_percentage_auto(v);
            match side {
                0 => style.inset.top = v,
                1 => style.inset.right = v,
                2 => style.inset.bottom = v,
                _ => style.inset.left = v,
            }
        }
    }

    if let Some(v) = obj.get("width") {
        style.size.width = parse_dimension(v);
    }
    if let Some(v) = obj.get("height") {
        style.size.height = parse_dimension(v);
    }
    if let Some(v) = obj.get("minWidth") {
        style.min_size.width = parse_length_percentage_auto(v);
    }
    if let Some(v) = obj.get("minHeight") {
        style.min_size.height = parse_length_percentage_auto(v);
    }
    if let Some(v) = obj.get("maxWidth") {
        style.max_size.width = parse_length_percentage_auto(v);
    }
    if let Some(v) = obj.get("maxHeight") {
        style.max_size.height = parse_length_percentage_auto(v);
    }
    if let Some(v) = obj.get("aspectRatio").and_then(number) {
        style.aspect_ratio = Some(v);
    }

    if let Some(v) = obj.get("flexGrow").and_then(number) {
        style.flex_grow = v;
    }
    if let Some(v) = obj.get("flexShrink").and_then(number) {
        style.flex_shrink = v;
    }
    if let Some(v) = obj.get("flexBasis") {
        style.flex_basis = parse_dimension(v);
    }

    if let Some(v) = obj.get("gridTemplateColumns") {
        style.grid_template_columns = parse_tracks(v);
    }
    if let Some(v) = obj.get("gridTemplateRows") {
        style.grid_template_rows = parse_tracks(v);
    }
    if let Some(v) = obj.get("gridColumn") {
        style.grid_column = parse_grid_placement(v);
    }
    if let Some(v) = obj.get("gridRow") {
        style.grid_row = parse_grid_placement(v);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tree() -> Layouts<()> {
        Layouts::new()
    }

    #[test]
    fn flex_row_places_children_with_gap() {
        let mut layouts = tree();
        layouts.create(
            1,
            parse_style(&json!({ "flexDirection": "row", "gap": 10 })),
            None,
        );
        layouts.create(2, parse_style(&json!({ "width": 50, "height": 50 })), None);
        layouts.create(3, parse_style(&json!({ "width": 50, "height": 50 })), None);
        layouts.insert(1, 2, None);
        layouts.insert(1, 3, None);
        layouts.compute(1, Some(300.0), Some(100.0), |_, _, _| Size::ZERO);
        assert_eq!(layouts.frame(2).unwrap().x, 0.0);
        assert_eq!(layouts.frame(3).unwrap().x, 60.0);
        assert_eq!(layouts.children(1), vec![2, 3]);
    }

    #[test]
    fn insert_before_and_move_keep_order() {
        let mut layouts = tree();
        for handle in 1..=4 {
            layouts.create(handle, Style::default(), None);
        }
        layouts.insert(1, 2, None);
        layouts.insert(1, 3, None);
        layouts.insert(1, 4, Some(2));
        assert_eq!(layouts.children(1), vec![4, 2, 3]);
        // Re-inserting moves rather than duplicates.
        layouts.insert(1, 4, None);
        assert_eq!(layouts.children(1), vec![2, 3, 4]);
        layouts.remove(1, 3);
        assert_eq!(layouts.children(1), vec![2, 4]);
    }

    #[test]
    fn grid_tracks_and_spans() {
        let mut layouts = tree();
        let grid = json!({ "display": "grid", "gridTemplateColumns": "100px 1fr", "gap": 10 });
        layouts.create(10, parse_style(&grid), None);
        layouts.create(11, parse_style(&json!({ "height": 50 })), None);
        layouts.create(12, parse_style(&json!({ "height": 50 })), None);
        layouts.create(
            13,
            parse_style(&json!({ "height": 20, "gridColumn": "span 2" })),
            None,
        );
        for child in [11, 12, 13] {
            layouts.insert(10, child, None);
        }
        layouts.compute(10, Some(310.0), None, |_, _, _| Size::ZERO);
        assert_eq!(layouts.frame(11).unwrap().width, 100.0);
        assert_eq!(layouts.frame(12).unwrap().x, 110.0);
        assert_eq!(layouts.frame(12).unwrap().width, 200.0);
        assert_eq!(layouts.frame(13).unwrap().width, 310.0);
        assert_eq!(layouts.frame(13).unwrap().y, 60.0);
    }

    #[test]
    fn leaves_are_measured_by_the_backend() {
        let mut layouts: Layouts<(f32, f32)> = Layouts::new();
        layouts.create(
            1,
            parse_style(&json!({ "flexDirection": "column", "padding": 8 })),
            None,
        );
        layouts.create(2, Style::default(), Some((80.0, 20.0)));
        layouts.insert(1, 2, None);
        layouts.compute(1, Some(200.0), None, |size, _, _| Size {
            width: size.0,
            height: size.1,
        });
        let frame = layouts.frame(2).unwrap();
        assert_eq!((frame.x, frame.y), (8.0, 8.0));
        // Stretch in a column: the leaf takes the container's inner width.
        assert_eq!(frame.width, 184.0);
        assert_eq!(frame.height, 20.0);
        assert_eq!(layouts.content_size(1).unwrap().1, 36.0);
    }

    #[test]
    fn style_parser_accepts_the_typescript_vocabulary() {
        let style = parse_style(&json!({
            "display": "grid",
            "gridTemplateColumns": "repeat(3, 1fr)",
            "gap": 16,
            "padding": [4, 8],
            "margin": { "top": 2 },
            "alignItems": "center",
            "minWidth": "50%",
            "flexGrow": 1,
            "gridRow": "1 / 3",
        }));
        assert_eq!(style.display, Display::Grid);
        assert_eq!(style.grid_template_columns.len(), 3);
        assert_eq!(style.gap.width, length(16.0));
        assert_eq!(style.padding.top, length(4.0));
        assert_eq!(style.padding.left, length(8.0));
        assert_eq!(style.margin.top, length(2.0));
        assert_eq!(style.margin.left, length(0.0));
        assert_eq!(style.align_items, Some(AlignItems::CENTER));
        assert_eq!(style.min_size.width, percent(0.5));
        assert_eq!(style.flex_grow, 1.0);
        assert_eq!(style.grid_row.start, line(1));
        assert_eq!(style.grid_row.end, line(3));
        assert_eq!(parse_grid_template_tracks("3").len(), 3);
    }
}
