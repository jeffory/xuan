//! Sidebar pane layout: the order, visibility, collapse state and height of
//! each pane in the right sidebar. The editor and plugins supply the panes;
//! this model only records how the user arranged them.
use serde::{Deserialize, Serialize};

/// The built-in Layers pane.
pub const LAYERS: &str = "layers";
/// The built-in Navigator pane, above Layers by default.
pub const NAVIGATOR: &str = "navigator";
/// Smallest body height a pane can be resized to.
pub const MIN_HEIGHT: f32 = 72.0;
/// Body height for a pane the user has not resized.
pub const DEFAULT_HEIGHT: f32 = 220.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pane {
    pub id: String,
    pub collapsed: bool,
    pub hidden: bool,
    /// Body height in points; zero means the default. Ignored for the pane
    /// that fills the remaining space.
    pub height: f32,
}

impl Default for Pane {
    fn default() -> Self {
        Self {
            id: String::new(),
            collapsed: false,
            hidden: false,
            height: 0.0,
        }
    }
}

impl Pane {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }

    pub fn body_height(&self) -> f32 {
        if self.height > 0.0 {
            self.height.max(MIN_HEIGHT)
        } else {
            DEFAULT_HEIGHT
        }
    }
}

/// Panes from top to bottom. Unknown identifiers (a pane from a plugin that is
/// not installed right now) are kept so the arrangement survives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Layout(pub Vec<Pane>);

impl Default for Layout {
    fn default() -> Self {
        Self(vec![Pane::new(NAVIGATOR), Pane::new(LAYERS)])
    }
}

impl Layout {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Drop duplicates and invalid sizes, and keep the built-in panes present.
    /// A layout saved before the Navigator existed gains it above Layers.
    pub fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.0
            .retain(|pane| !pane.id.is_empty() && seen.insert(pane.id.clone()));
        for pane in &mut self.0 {
            if !pane.height.is_finite() || pane.height < 0.0 {
                pane.height = 0.0;
            }
        }
        let layers = self.ensure(LAYERS);
        if self.get(NAVIGATOR).is_none() {
            self.0.insert(layers, Pane::new(NAVIGATOR));
        }
    }

    /// Add a pane at the bottom when the layout does not know it yet.
    pub fn ensure(&mut self, id: &str) -> usize {
        match self.0.iter().position(|pane| pane.id == id) {
            Some(index) => index,
            None => {
                self.0.push(Pane::new(id));
                self.0.len() - 1
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Pane> {
        self.0.iter().find(|pane| pane.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Pane> {
        self.0.iter_mut().find(|pane| pane.id == id)
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.0.iter().position(|pane| pane.id == id)
    }

    /// Move the pane at `from` so it lands before the pane currently at `to`
    /// (`to == len` moves it to the bottom). Returns whether anything changed.
    pub fn move_pane(&mut self, from: usize, to: usize) -> bool {
        if from >= self.0.len() || to > self.0.len() || to == from || to == from + 1 {
            return false;
        }
        let pane = self.0.remove(from);
        let to = if to > from { to - 1 } else { to };
        self.0.insert(to, pane);
        true
    }

    pub fn toggle_collapsed(&mut self, id: &str) {
        if let Some(pane) = self.get_mut(id) {
            pane.collapsed = !pane.collapsed;
        }
    }

    pub fn set_hidden(&mut self, id: &str, hidden: bool) {
        let index = self.ensure(id);
        self.0[index].hidden = hidden;
    }

    pub fn set_height(&mut self, id: &str, height: f32) {
        if let Some(pane) = self.get_mut(id) {
            pane.height = height.max(MIN_HEIGHT);
        }
    }

    pub fn visible<'a>(
        &'a self,
        known: &'a dyn Fn(&str) -> bool,
    ) -> impl Iterator<Item = &'a Pane> {
        self.0
            .iter()
            .filter(move |pane| !pane.hidden && known(&pane.id))
    }

    /// The expanded pane that takes the space left over by the others: Layers
    /// when it is expanded, otherwise the lowest expanded pane.
    pub fn fill_pane<'a>(&'a self, known: &'a dyn Fn(&str) -> bool) -> Option<&'a str> {
        let expanded: Vec<&'a Pane> = self.visible(known).filter(|p| !p.collapsed).collect();
        expanded
            .iter()
            .find(|pane| pane.id == LAYERS)
            .or(expanded.last())
            .map(|pane| pane.id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn reordering_moves_panes_around_their_neighbours() {
        let mut layout = Layout(vec![Pane::new("a"), Pane::new("b"), Pane::new("c")]);
        assert!(!layout.move_pane(0, 0));
        assert!(!layout.move_pane(0, 1));
        assert!(layout.move_pane(0, 3));
        assert_eq!(ids(&layout), ["b", "c", "a"]);
        assert!(layout.move_pane(2, 0));
        assert_eq!(ids(&layout), ["a", "b", "c"]);
        assert!(layout.move_pane(2, 1));
        assert_eq!(ids(&layout), ["a", "c", "b"]);
        assert!(!layout.move_pane(5, 0));
        assert!(!layout.move_pane(0, 9));
    }

    #[test]
    fn layers_fills_unless_collapsed_and_hidden_panes_are_skipped() {
        let mut layout = Layout(vec![Pane::new("histogram"), Pane::new(LAYERS)]);
        assert_eq!(layout.fill_pane(&all), Some(LAYERS));
        layout.toggle_collapsed(LAYERS);
        assert_eq!(layout.fill_pane(&all), Some("histogram"));
        layout.set_hidden("histogram", true);
        assert_eq!(layout.fill_pane(&all), None);
        assert_eq!(layout.visible(&all).count(), 1);
        layout.set_hidden("jobs", false);
        assert_eq!(ids(&layout), ["histogram", LAYERS, "jobs"]);
        let known = |id: &str| id != "jobs";
        assert_eq!(layout.visible(&known).count(), 1);
    }

    #[test]
    fn sanitize_removes_duplicates_and_restores_layers() {
        let mut layout = Layout(vec![
            Pane::new("a"),
            Pane {
                id: "a".into(),
                height: f32::NAN,
                ..Pane::default()
            },
            Pane {
                id: "b".into(),
                height: -4.0,
                ..Pane::default()
            },
            Pane::new(""),
        ]);
        layout.sanitize();
        assert_eq!(ids(&layout), ["a", "b", NAVIGATOR, LAYERS]);
        assert!(layout.0.iter().all(|pane| pane.height == 0.0));
        assert_eq!(layout.get("b").unwrap().body_height(), DEFAULT_HEIGHT);
        layout.set_height("b", 10.0);
        assert_eq!(layout.get("b").unwrap().body_height(), MIN_HEIGHT);
        layout.reset();
        assert_eq!(layout, Layout::default());
        assert_eq!(ids(&layout), [NAVIGATOR, LAYERS]);
        // A hidden or moved Navigator stays where the user put it.
        let mut layout = Layout(vec![Pane::new(LAYERS), Pane::new("x")]);
        layout.sanitize();
        assert_eq!(ids(&layout), [NAVIGATOR, LAYERS, "x"]);
        layout.set_hidden(NAVIGATOR, true);
        assert!(layout.move_pane(0, 3));
        let saved = layout.clone();
        layout.sanitize();
        assert_eq!(layout, saved);
    }

    #[test]
    fn layout_serializes_as_a_plain_array() {
        let mut layout = Layout::default();
        layout.set_hidden("histogram", true);
        layout.set_height(LAYERS, 300.0);
        let text = toml::to_string(&Holder {
            panes: layout.clone(),
        })
        .unwrap();
        assert!(text.contains("[[panes]]"), "{text}");
        let back: Holder = toml::from_str(&text).unwrap();
        assert_eq!(back.panes, layout);
        // A layout saved before the Navigator existed gains it on load.
        let mut partial: Holder = toml::from_str("[[panes]]\nid = 'layers'\n").unwrap();
        partial.panes.sanitize();
        assert_eq!(partial.panes, Layout::default());
    }

    #[derive(Serialize, Deserialize)]
    struct Holder {
        panes: Layout,
    }

    fn ids(layout: &Layout) -> Vec<&str> {
        layout.0.iter().map(|pane| pane.id.as_str()).collect()
    }
}
