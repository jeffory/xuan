//! The declarative widget tree a plugin pane returns. The editor draws it;
//! the plugin only describes it.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Nesting deeper than this is cut off to keep a broken plugin from
/// exhausting the layout.
pub const MAX_DEPTH: usize = 16;
/// Nodes in one tree, counting list items, select options and swatches.
pub const MAX_NODES: usize = 2000;
/// Longest text, in bytes, of a label, button, value and the like.
pub const MAX_TEXT: usize = 4096;
/// Longest widget or item id, option id, color or suffix, in bytes.
pub const MAX_ID: usize = 256;
/// Longest link, in bytes.
pub const MAX_URL: usize = 2048;
/// Largest size, in points, of an image, text box, space or gap.
pub const MAX_SIZE: f32 = 4096.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Column {
        #[serde(default)]
        children: Vec<Node>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gap: Option<f32>,
    },
    Row {
        #[serde(default)]
        children: Vec<Node>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gap: Option<f32>,
    },
    Heading {
        text: String,
    },
    Label {
        text: String,
        #[serde(default)]
        muted: bool,
        #[serde(default)]
        small: bool,
        #[serde(default = "yes")]
        wrap: bool,
    },
    Separator,
    Space {
        #[serde(default = "default_space")]
        size: f32,
    },
    Button {
        id: String,
        label: String,
        #[serde(default)]
        primary: bool,
        #[serde(default = "yes")]
        enabled: bool,
        /// Text the host copies to the clipboard when the user clicks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        copy: Option<String>,
    },
    Checkbox {
        id: String,
        label: String,
        #[serde(default)]
        value: bool,
    },
    Text {
        id: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        multiline: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f32>,
    },
    Number {
        id: String,
        #[serde(default)]
        value: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
        #[serde(default)]
        suffix: String,
        #[serde(default)]
        integer: bool,
    },
    Slider {
        id: String,
        #[serde(default)]
        value: f64,
        min: f64,
        max: f64,
        #[serde(default)]
        label: String,
        #[serde(default)]
        suffix: String,
        #[serde(default)]
        logarithmic: bool,
    },
    Select {
        id: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        options: Vec<Option_>,
    },
    Color {
        id: String,
        #[serde(default = "white")]
        value: String,
    },
    Image {
        src: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<f32>,
        #[serde(default = "yes")]
        fit: bool,
    },
    Progress {
        #[serde(default)]
        value: Option<f32>,
        #[serde(default)]
        label: String,
    },
    List {
        id: String,
        #[serde(default)]
        items: Vec<Item>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected: Option<String>,
    },
    Swatches {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        colors: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected: Option<String>,
    },
    Link {
        label: String,
        url: String,
    },
}

fn yes() -> bool {
    true
}

fn default_space() -> f32 {
    8.0
}

fn white() -> String {
    "#ffffff".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Option_ {
    pub id: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// What the user did to a widget, sent back as `pane/render` with `reason = "event"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub widget: String,
    pub value: Value,
}

impl Node {
    pub fn empty() -> Self {
        Self::Column {
            children: Vec::new(),
            gap: None,
        }
    }

    /// Parse a tree, cutting off excessive nesting, node counts, text
    /// lengths and sizes.
    pub fn parse(value: Value) -> anyhow::Result<Self> {
        let mut node: Self = serde_json::from_value(value)?;
        let mut budget = MAX_NODES;
        node.limit(0, &mut budget);
        Ok(node)
    }

    fn limit(&mut self, depth: usize, budget: &mut usize) {
        *budget = budget.saturating_sub(1);
        match self {
            Self::Column { children, gap } | Self::Row { children, gap } => {
                size(gap);
                if depth >= MAX_DEPTH || *budget == 0 {
                    children.clear();
                }
                children.truncate(*budget);
                for child in children.iter_mut() {
                    child.limit(depth + 1, budget);
                }
            }
            Self::Heading { text } | Self::Label { text, .. } => clip(text, MAX_TEXT),
            Self::Separator => {}
            Self::Space { size } => *size = clamp(*size),
            Self::Button {
                id, label, copy, ..
            } => {
                clip(id, MAX_ID);
                clip(label, MAX_TEXT);
                if let Some(copy) = copy {
                    clip(copy, MAX_TEXT);
                }
            }
            Self::Checkbox { id, label, .. } => {
                clip(id, MAX_ID);
                clip(label, MAX_TEXT);
            }
            Self::Text {
                id,
                value,
                placeholder,
                width,
                ..
            } => {
                clip(id, MAX_ID);
                clip(value, MAX_TEXT);
                clip(placeholder, MAX_TEXT);
                size(width);
            }
            Self::Number {
                id,
                value,
                min,
                max,
                step,
                suffix,
                ..
            } => {
                clip(id, MAX_ID);
                clip(suffix, MAX_ID);
                *value = finite(*value);
                for bound in [min, max, step] {
                    *bound = bound.filter(|v| v.is_finite());
                }
            }
            Self::Slider {
                id,
                value,
                min,
                max,
                label,
                suffix,
                ..
            } => {
                clip(id, MAX_ID);
                clip(label, MAX_TEXT);
                clip(suffix, MAX_ID);
                *value = finite(*value);
                *min = finite(*min);
                *max = finite(*max);
            }
            Self::Select { id, value, options } => {
                clip(id, MAX_ID);
                clip(value, MAX_ID);
                options.truncate(*budget);
                *budget -= options.len();
                for option in options {
                    clip(&mut option.id, MAX_ID);
                    clip(&mut option.label, MAX_TEXT);
                }
            }
            Self::Color { id, value } => {
                clip(id, MAX_ID);
                clip(value, MAX_ID);
            }
            Self::Image { width, height, .. } => {
                size(width);
                size(height);
            }
            Self::Progress { value, label } => {
                *value = value.filter(|v| v.is_finite());
                clip(label, MAX_TEXT);
            }
            Self::List {
                id,
                items,
                selected,
            } => {
                clip(id, MAX_ID);
                if let Some(selected) = selected {
                    clip(selected, MAX_ID);
                }
                items.truncate(*budget);
                *budget -= items.len();
                for item in items {
                    clip(&mut item.id, MAX_ID);
                    clip(&mut item.label, MAX_TEXT);
                    clip(&mut item.detail, MAX_TEXT);
                }
            }
            Self::Swatches {
                id,
                colors,
                selected,
            } => {
                for text in id.iter_mut().chain(selected.iter_mut()) {
                    clip(text, MAX_ID);
                }
                colors.truncate(*budget);
                *budget -= colors.len();
                for color in colors {
                    clip(color, MAX_ID);
                }
            }
            Self::Link { label, url } => {
                clip(label, MAX_TEXT);
                clip(url, MAX_URL);
            }
        }
    }

    /// Parse `#rrggbb` or `#rrggbbaa`.
    pub fn color(text: &str) -> Option<[u8; 4]> {
        let hex = text.strip_prefix('#')?;
        let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        match hex.len() {
            6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
            8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
            _ => None,
        }
    }

    pub fn color_text(color: [u8; 4]) -> String {
        if color[3] == 255 {
            format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
        } else {
            format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                color[0], color[1], color[2], color[3]
            )
        }
    }
}

/// Cut `text` to at most `max` bytes on a character boundary.
fn clip(text: &mut String, max: usize) {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

fn finite(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

fn clamp(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(0.0, MAX_SIZE)
    } else {
        0.0
    }
}

fn size(size: &mut Option<f32>) {
    *size = size.filter(|s| s.is_finite()).map(clamp);
}

/// Decode standard or URL-safe base64, ignoring whitespace and padding.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base64_decodes_standard_and_url_safe_text() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVsbG8").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVs\nbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(decode_base64("-_8=").unwrap(), [0xfb, 0xff]);
        assert!(decode_base64("a*b").is_none());
        assert_eq!(decode_base64("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn trees_parse_with_defaults_and_limits() {
        let tree = Node::parse(json!({
            "type": "column",
            "children": [
                {"type": "heading", "text": "Histogram"},
                {"type": "image", "src": "hist.png", "height": 120},
                {"type": "row", "children": [
                    {"type": "button", "id": "refresh", "label": "Refresh"},
                    {"type": "checkbox", "id": "log", "label": "Log scale", "value": true}
                ]},
                {"type": "select", "id": "channel", "value": "rgb", "options": [{"id": "rgb"}, {"id": "r", "label": "Red"}]},
                {"type": "progress"},
                {"type": "space"}
            ]
        }))
        .unwrap();
        let Node::Column { children, .. } = &tree else {
            panic!()
        };
        assert_eq!(children.len(), 6);
        assert!(
            matches!(&children[1], Node::Image { fit: true, height: Some(h), .. } if *h == 120.0)
        );
        assert!(matches!(&children[4], Node::Progress { value: None, .. }));
        assert!(matches!(&children[5], Node::Space { size } if *size == 8.0));
        assert!(Node::parse(json!({"type": "spinner"})).is_err());
        assert!(Node::parse(json!({"type": "button", "id": "x"})).is_err());

        let mut deep = json!({"type": "label", "text": "leaf"});
        for _ in 0..40 {
            deep = json!({"type": "column", "children": [deep]});
        }
        let mut node = Node::parse(deep).unwrap();
        let mut depth = 0;
        while let Node::Column { children, .. } = node {
            depth += 1;
            match children.into_iter().next() {
                Some(child) => node = child,
                None => break,
            }
        }
        assert!(depth <= MAX_DEPTH + 1);

        let wide = json!({"type": "row", "children": (0..3000).map(|_| json!({"type": "separator"})).collect::<Vec<_>>()});
        let Node::Row { children, .. } = Node::parse(wide).unwrap() else {
            panic!()
        };
        assert!(children.len() < MAX_NODES);
    }

    #[test]
    fn collections_count_against_the_node_budget_and_text_is_capped() {
        let items: Vec<_> = (0..5000)
            .map(|i| json!({"id": i.to_string(), "label": "x"}))
            .collect();
        let options: Vec<_> = (0..5000).map(|i| json!({"id": i.to_string()})).collect();
        let colors: Vec<_> = (0..5000).map(|_| json!("#ffffff")).collect();
        let tree = Node::parse(json!({"type": "column", "children": [
            {"type": "list", "id": "l", "items": items},
            {"type": "select", "id": "s", "options": options},
            {"type": "swatches", "colors": colors},
            {"type": "label", "text": "é".repeat(MAX_TEXT)},
            {"type": "button", "id": "b".repeat(5000), "label": "go", "copy": "c".repeat(9000)},
            {"type": "image", "src": "a.png", "width": 1e9, "height": -5},
            {"type": "space", "size": 1e30},
            {"type": "link", "label": "l", "url": format!("https://x.example/{}", "a".repeat(9000))},
        ]}))
        .unwrap();
        let Node::Column { children, .. } = &tree else {
            panic!()
        };
        let mut counted = children.len() + 1;
        for child in children {
            counted += match child {
                Node::List { items, .. } => items.len(),
                Node::Select { options, .. } => options.len(),
                Node::Swatches { colors, .. } => colors.len(),
                _ => 0,
            };
        }
        // Siblings already kept may each overshoot by their own node.
        assert!(counted <= MAX_NODES + children.len(), "{counted}");
        assert!(matches!(&children[0], Node::List { items, .. } if !items.is_empty()));
        assert!(
            matches!(&children[3], Node::Label { text, .. } if text.len() <= MAX_TEXT && text.len() > MAX_TEXT - 2)
        );
        assert!(
            matches!(&children[4], Node::Button { id, copy: Some(copy), .. }
            if id.len() == MAX_ID && copy.len() == MAX_TEXT)
        );
        assert!(matches!(
            &children[5],
            Node::Image { width: Some(w), height: Some(h), .. } if *w == MAX_SIZE && *h == 0.0
        ));
        assert!(matches!(&children[6], Node::Space { size } if *size == MAX_SIZE));
        assert!(matches!(&children[7], Node::Link { url, .. } if url.len() == MAX_URL));
    }

    #[test]
    fn colors_roundtrip() {
        assert_eq!(Node::color("#1e90ff"), Some([30, 144, 255, 255]));
        assert_eq!(Node::color("#1e90ff80"), Some([30, 144, 255, 128]));
        assert_eq!(Node::color("1e90ff"), None);
        assert_eq!(Node::color("#12"), None);
        assert_eq!(Node::color_text([30, 144, 255, 255]), "#1e90ff");
        assert_eq!(Node::color_text([30, 144, 255, 128]), "#1e90ff80");
    }
}
